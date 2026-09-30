//! 显式部署配置；不提供可任意修改供应商地址的运行时入口。
use personal_ai_agent_core::{
    budget::{CostReservation, TokenPrices},
    reply_executor::{BudgetedReplyExecutor, ReplySender},
};
use personal_ai_llm_openai::replies::{OpenAiReplies, OpenAiReplyPolicy, REPLY_MODEL, ReplyPrices};
use personal_ai_storage::{
    replies::{ReplyConfiguration, ReplyContext},
    reply_budgets::{ReplyBudget, ReplyBudgetPlanner, ReplyDispatchStore},
};
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) struct PaidReplies {
    pub configuration: ReplyConfiguration,
    pub budget: ReplyBudget,
    pub planner: Arc<dyn ReplyBudgetPlanner>,
    pub executor: BudgetedReplyExecutor,
    pub reservation: i64,
}
impl PaidReplies {
    pub fn quote(&self) -> Value {
        json!({"configuration_revision": self.configuration.revision, "currency": self.budget.currency,
            "reservation_micro": self.reservation.to_string(), "request_limit_micro": self.budget.request_limit.to_string(), "daily_limit_micro": self.budget.daily_limit.to_string()})
    }
    pub fn new(
        store: Arc<dyn ReplyDispatchStore>,
        configuration: ReplyConfiguration,
        budget: ReplyBudget,
        planner: Arc<dyn ReplyBudgetPlanner>,
        sender: Arc<dyn ReplySender>,
    ) -> Result<Self, String> {
        let quote = CostReservation::quote(
            TokenPrices {
                input_per_million: budget.input_price_per_million,
                output_per_million: budget.output_price_per_million,
            },
            budget.input_token_bound,
            budget.output_token_bound,
            budget.request_limit,
        )
        .map_err(|_| "invalid reply limits")?;
        Ok(Self {
            executor: BudgetedReplyExecutor::new(store, sender, configuration.clone()),
            configuration,
            budget,
            planner,
            reservation: quote.amount(),
        })
    }
}

struct Settings {
    configuration: ReplyConfiguration,
    prices: ReplyPrices,
    until: i64,
    key: String,
}
impl Settings {
    fn read(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let required = |name: &str| {
            get(name)
                .filter(|v| !v.trim().is_empty())
                .ok_or_else(|| format!("{name} is required for openai replies"))
        };
        let integer = |name: &str| -> Result<i64, String> {
            let raw = required(name)?;
            if !raw.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("{name} must be a positive integer"));
            }
            raw.parse::<i64>()
                .ok()
                .filter(|v| *v > 0)
                .ok_or_else(|| format!("{name} must be a positive integer"))
        };
        Ok(Self {
            configuration: ReplyConfiguration {
                model: REPLY_MODEL.into(),
                revision: required("REPLY_CONFIGURATION_REVISION")?,
            },
            prices: ReplyPrices {
                version: required("REPLY_PRICE_VERSION")?,
                input_per_million: u64::try_from(integer("REPLY_INPUT_PRICE_MICRO_PER_MILLION")?)
                    .map_err(|_| "invalid input price")?,
                output_per_million: u64::try_from(integer("REPLY_OUTPUT_PRICE_MICRO_PER_MILLION")?)
                    .map_err(|_| "invalid output price")?,
                request_limit: integer("REPLY_REQUEST_LIMIT_MICRO")?,
                daily_limit: integer("REPLY_DAILY_LIMIT_MICRO")?,
            },
            until: integer("REPLY_PRICE_VALID_UNTIL_UNIX_MS")?,
            key: required("REPLY_OPENAI_API_KEY")?,
        })
    }
}

pub(crate) async fn from_env(store: Arc<dyn ReplyDispatchStore>) -> Result<PaidReplies, String> {
    let settings = Settings::read(|key| std::env::var(key).ok())?;
    let policy = Arc::new(
        OpenAiReplyPolicy::new(settings.configuration.clone(), settings.prices)
            .map_err(|_| "invalid reply provider configuration")?,
    );
    // 固定策略的输入上界与文本长度无关；实际冻结上下文在预留事务内再次完整核验。
    let budget = policy
        .plan(&ReplyContext {
            system: "configuration validation".into(),
            user_messages: vec!["validation".into()],
            first_sequence: 1,
            max_output_tokens: 1024,
            configuration: settings.configuration.clone(),
        })
        .map_err(|_| "invalid reply budget")?;
    let sender = Arc::new(
        OpenAiReplies::new(&settings.key, policy.clone())
            .map_err(|_| "invalid reply credentials")?,
    );
    store.register_reply_configuration(&settings.configuration, &budget, settings.until).await.map_err(|_| "reply configuration registration rejected (expired, disabled, changed, or unavailable)")?;
    PaidReplies::new(store, settings.configuration, budget, policy, sender)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paid_settings_require_explicit_valid_values_without_exposing_secrets() {
        let base = |key: &str| {
            Some(
                match key {
                    "REPLY_CONFIGURATION_REVISION" => "test-v1",
                    "REPLY_PRICE_VERSION" => "price-test",
                    "REPLY_PRICE_VALID_UNTIL_UNIX_MS" => "4102444800000",
                    "REPLY_OPENAI_API_KEY" => "test-secret",
                    _ => "1000000",
                }
                .to_string(),
            )
        };
        assert!(Settings::read(base).is_ok());
        for name in [
            "REPLY_CONFIGURATION_REVISION",
            "REPLY_PRICE_VERSION",
            "REPLY_PRICE_VALID_UNTIL_UNIX_MS",
            "REPLY_OPENAI_API_KEY",
            "REPLY_INPUT_PRICE_MICRO_PER_MILLION",
            "REPLY_OUTPUT_PRICE_MICRO_PER_MILLION",
            "REPLY_REQUEST_LIMIT_MICRO",
            "REPLY_DAILY_LIMIT_MICRO",
        ] {
            assert!(Settings::read(|k| if k == name { None } else { base(k) }).is_err());
        }
        for value in ["0", "-1", "1.2", "999999999999999999999", "test-secret"] {
            let error = Settings::read(|k| {
                if k == "REPLY_DAILY_LIMIT_MICRO" {
                    Some(value.into())
                } else {
                    base(k)
                }
            })
            .err()
            .unwrap();
            assert!(!error.contains("test-secret"));
        }
    }
}
