//! 部署者显式登记两阶段费用配置，默认关闭，启动不产生模型调用。
use crate::ModelAgentRuntime;
use personal_ai_agent_core::model_executor::ModelAgentExecutor;
use personal_ai_llm_openai::{
    agents::{
        AGENT_CHAT_COUNTER, AGENT_CHAT_INPUT_BOUND, AGENT_EMBEDDING_COUNTER,
        AGENT_EMBEDDING_DIMENSIONS, AGENT_EMBEDDING_INPUT_BOUND, AGENT_EMBEDDING_MODEL,
        OpenAiAgentModels,
    },
    replies::REPLY_MODEL,
};
use personal_ai_storage::{
    model_agents::{
        AgentBudgetLimits, ModelCallBudget, ModelPlanningConfiguration, ModelPlanningStore,
    },
    model_execution::{ModelExecutionConfiguration, ModelExecutionStore},
};
use personal_ai_storage_postgres::PostgresStore;
use std::sync::Arc;

struct Settings {
    planning: ModelPlanningConfiguration,
    execution: ModelExecutionConfiguration,
    key: String,
}
impl Settings {
    fn read(get: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, String> {
        match get("MODEL_AGENT_MODE").as_deref() {
            None | Some("disabled") => return Ok(None),
            Some("openai") => (),
            _ => return Err("MODEL_AGENT_MODE must be disabled or openai".into()),
        }
        let required = |name: &str| {
            get(name)
                .filter(|v| !v.trim().is_empty())
                .ok_or_else(|| format!("{name} is required for model agents"))
        };
        let integer = |name: &str| -> Result<i64, String> {
            let raw = required(name)?;
            if !raw.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("{name} must be a positive integer"));
            }
            raw.parse::<i64>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("{name} must be a positive integer"))
        };
        if get("KNOWLEDGE_INDEX_ENABLED").as_deref() != Some("true")
            || required("OPENAI_EMBEDDING_MODEL")? != AGENT_EMBEDDING_MODEL
            || integer("EMBEDDING_DIMENSIONS")?
                != i64::try_from(AGENT_EMBEDDING_DIMENSIONS)
                    .map_err(|_| "invalid embedding dimensions")?
            || get("OPENAI_BASE_URL")
                .is_some_and(|s| s.trim_end_matches('/') != "https://api.openai.com/v1")
        {
            return Err(
                "model agents require the official matching knowledge embedding configuration"
                    .into(),
            );
        }
        let version = required("MODEL_AGENT_CONFIGURATION_VERSION")?;
        if version.len() > 110 || !version.bytes().all(|b| b.is_ascii_graphic()) {
            return Err("invalid MODEL_AGENT_CONFIGURATION_VERSION".into());
        }
        let until = integer("MODEL_AGENT_PRICE_VALID_UNTIL_UNIX_MS")?;
        let limits = AgentBudgetLimits {
            phase_amount: integer("MODEL_AGENT_PHASE_LIMIT_MICRO")?,
            daily_amount: integer("MODEL_AGENT_DAILY_LIMIT_MICRO")?,
            daily_model_calls: u32::try_from(integer("MODEL_AGENT_DAILY_MODEL_CALLS")?)
                .map_err(|_| "invalid MODEL_AGENT_DAILY_MODEL_CALLS")?,
            daily_tool_calls: u32::try_from(integer("MODEL_AGENT_DAILY_TOOL_CALLS")?)
                .map_err(|_| "invalid MODEL_AGENT_DAILY_TOOL_CALLS")?,
        };
        let planning = ModelPlanningConfiguration {
            limits,
            budget: ModelCallBudget {
                configuration_version: format!("{version}-planning"),
                provider: "openai".into(),
                model: REPLY_MODEL.into(),
                currency: "USD".into(),
                price_version: required("MODEL_AGENT_CHAT_PRICE_VERSION")?,
                counter_version: AGENT_CHAT_COUNTER.into(),
                input_price_per_million: u64::try_from(integer(
                    "MODEL_AGENT_CHAT_INPUT_PRICE_MICRO_PER_MILLION",
                )?)
                .map_err(|_| "invalid chat input price")?,
                output_price_per_million: u64::try_from(integer(
                    "MODEL_AGENT_CHAT_OUTPUT_PRICE_MICRO_PER_MILLION",
                )?)
                .map_err(|_| "invalid chat output price")?,
                input_token_bound: AGENT_CHAT_INPUT_BOUND,
                output_token_bound: 2048,
                valid_until_unix_ms: until,
            },
        };
        let mut answer = planning.budget.clone();
        answer.configuration_version = format!("{version}-answer");
        answer.output_token_bound = 1024;
        let embedding = ModelCallBudget {
            configuration_version: format!("{version}-embedding"),
            model: AGENT_EMBEDDING_MODEL.into(),
            counter_version: AGENT_EMBEDDING_COUNTER.into(),
            price_version: required("MODEL_AGENT_EMBEDDING_PRICE_VERSION")?,
            input_price_per_million: u64::try_from(integer(
                "MODEL_AGENT_EMBEDDING_PRICE_MICRO_PER_MILLION",
            )?)
            .map_err(|_| "invalid embedding price")?,
            output_price_per_million: 0,
            input_token_bound: AGENT_EMBEDDING_INPUT_BOUND,
            output_token_bound: 0,
            ..planning.budget.clone()
        };
        Ok(Some(Self {
            planning,
            execution: ModelExecutionConfiguration {
                version: answer.configuration_version.clone(),
                answer,
                embedding,
                limits,
            },
            key: required("MODEL_AGENT_OPENAI_API_KEY")?,
        }))
    }
}
impl ModelAgentRuntime {
    /// 默认关闭仍支持读取历史及取消；显式启用时检查索引目标和冻结费用配置。
    /// # Errors
    /// 配置缺失、索引不兼容、价格过期/停用或仓储不可用时拒绝启动。
    pub async fn from_env(store: Arc<PostgresStore>) -> Result<Arc<Self>, String> {
        let Some(settings) = Settings::read(|key| std::env::var(key).ok())? else {
            return Ok(Arc::new(Self {
                store,
                executor: None,
                planning_version: String::new(),
                execution_version: String::new(),
            }));
        };
        let required = |key: &str| {
            std::env::var(key)
                .ok()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| format!("{key} is required for model agents"))
        };
        let vectors = personal_ai_storage_qdrant::QdrantStore::new(
            &required("QDRANT_URL")?,
            &required("QDRANT_COLLECTION")?,
            std::env::var("QDRANT_API_KEY")
                .ok()
                .filter(|v| !v.is_empty())
                .as_deref(),
            AGENT_EMBEDDING_MODEL,
            AGENT_EMBEDDING_DIMENSIONS,
        )
        .map_err(|_| "invalid model agent vector configuration")?;
        vectors
            .ensure_collection()
            .await
            .map_err(|_| "model agent vector collection unavailable")?;
        let retriever = personal_ai_knowledge::model_retrieval::KnowledgeModelRetriever::new(
            store.clone(),
            Arc::new(vectors),
            AGENT_EMBEDDING_MODEL.into(),
            AGENT_EMBEDDING_DIMENSIONS,
        )
        .map_err(|_| "invalid model retriever")?;
        let provider = OpenAiAgentModels::new(
            &settings.key,
            settings.planning.budget.clone(),
            settings.execution.clone(),
        )
        .map_err(|_| "invalid model agent provider configuration")?;
        store
            .register_model_planning_configuration(&settings.planning)
            .await
            .map_err(|_| "model planning configuration registration rejected")?;
        store
            .register_model_execution_configuration(&settings.execution)
            .await
            .map_err(|_| "model execution configuration registration rejected")?;
        Ok(Arc::new(Self {
            planning_version: settings.planning.budget.configuration_version.clone(),
            execution_version: settings.execution.version.clone(),
            executor: Some(Arc::new(ModelAgentExecutor::new(
                store.clone(),
                Arc::new(provider),
                Arc::new(retriever),
                settings.planning,
                settings.execution,
            ))),
            store,
        }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_agent_settings_default_disabled_and_require_explicit_matching_values() {
        assert!(Settings::read(|_| None).unwrap().is_none());
        let base = |key: &str| {
            Some(
                match key {
                    "MODEL_AGENT_MODE" => "openai",
                    "KNOWLEDGE_INDEX_ENABLED" => "true",
                    "OPENAI_EMBEDDING_MODEL" => AGENT_EMBEDDING_MODEL,
                    "EMBEDDING_DIMENSIONS" => "1536",
                    "OPENAI_BASE_URL" => "https://api.openai.com/v1",
                    "MODEL_AGENT_OPENAI_API_KEY" => "secret-fixture",
                    "MODEL_AGENT_CONFIGURATION_VERSION" => "fixture",
                    "MODEL_AGENT_CHAT_PRICE_VERSION" | "MODEL_AGENT_EMBEDDING_PRICE_VERSION" => {
                        "price-v1"
                    }
                    "MODEL_AGENT_PRICE_VALID_UNTIL_UNIX_MS" => "4102444800000",
                    "MODEL_AGENT_DAILY_TOOL_CALLS" => "100",
                    _ => "1000000",
                }
                .to_owned(),
            )
        };
        assert!(Settings::read(base).unwrap().is_some());
        for key in [
            "MODEL_AGENT_OPENAI_API_KEY",
            "MODEL_AGENT_CONFIGURATION_VERSION",
            "MODEL_AGENT_CHAT_PRICE_VERSION",
            "MODEL_AGENT_CHAT_INPUT_PRICE_MICRO_PER_MILLION",
            "MODEL_AGENT_CHAT_OUTPUT_PRICE_MICRO_PER_MILLION",
            "MODEL_AGENT_EMBEDDING_PRICE_VERSION",
            "MODEL_AGENT_EMBEDDING_PRICE_MICRO_PER_MILLION",
            "MODEL_AGENT_PHASE_LIMIT_MICRO",
            "MODEL_AGENT_DAILY_LIMIT_MICRO",
            "MODEL_AGENT_DAILY_MODEL_CALLS",
            "MODEL_AGENT_DAILY_TOOL_CALLS",
            "MODEL_AGENT_PRICE_VALID_UNTIL_UNIX_MS",
        ] {
            assert!(Settings::read(|k| if k == key { None } else { base(k) }).is_err());
        }
        for (key, value) in [
            ("MODEL_AGENT_MODE", "fixture"),
            ("KNOWLEDGE_INDEX_ENABLED", "false"),
            ("OPENAI_BASE_URL", "https://other.invalid/v1"),
            ("EMBEDDING_DIMENSIONS", "2"),
            ("MODEL_AGENT_DAILY_LIMIT_MICRO", "secret-fixture"),
        ] {
            let error = Settings::read(|k| {
                if k == key {
                    Some(value.into())
                } else {
                    base(k)
                }
            })
            .err()
            .unwrap();
            assert!(!error.contains("secret-fixture"));
        }
    }
}
