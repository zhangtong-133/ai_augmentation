//! 两阶段 Agent 的官方固定模型适配器。价格由部署端提供，不读取环境或自动启用。
use crate::replies::{REPLY_MODEL, decode};
use personal_ai_agent_core::{
    BoxFuture,
    model_executor::{ModelAgentProvider, ModelChatStage, ModelEmbeddingCompletion},
    model_plan::model_search_schema,
    reply_executor::{ReplyCompletion, ReplySendError},
};
use personal_ai_llm::{ChatRequest, Embedding, Role};
use personal_ai_storage::{
    model_agents::ModelCallBudget,
    model_execution::ModelExecutionConfiguration,
    reply_budgets::{ReplyBudget, ReplyUsage},
};
use reqwest::{
    Client, Url,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const AGENT_CHAT_COUNTER: &str = "o200k-0.12.1-agent-context-v1";
pub const AGENT_EMBEDDING_COUNTER: &str = "cl100k-0.12.1-8192-d1536-v1";
pub const AGENT_EMBEDDING_MODEL: &str = "text-embedding-3-small";
pub const AGENT_EMBEDDING_DIMENSIONS: usize = 1536;
pub const AGENT_CHAT_INPUT_BOUND: u64 = 128_000;
pub const AGENT_EMBEDDING_INPUT_BOUND: u64 = 8192;

pub struct OpenAiAgentModels {
    client: Client,
    base: Url,
    planning: ModelCallBudget,
    execution: ModelExecutionConfiguration,
    chat_tokens: tiktoken_rs::CoreBPE,
    embedding_tokens: tiktoken_rs::CoreBPE,
    disabled: AtomicBool,
}
fn invalid() -> ReplySendError {
    ReplySendError::InvalidConfiguration
}
fn valid_budget(b: &ModelCallBudget, embedding: bool, output: u64) -> bool {
    b.provider == "openai"
        && b.currency == "USD"
        && b.model
            == if embedding {
                AGENT_EMBEDDING_MODEL
            } else {
                REPLY_MODEL
            }
        && b.counter_version
            == if embedding {
                AGENT_EMBEDDING_COUNTER
            } else {
                AGENT_CHAT_COUNTER
            }
        && b.input_token_bound
            == if embedding {
                AGENT_EMBEDDING_INPUT_BOUND
            } else {
                AGENT_CHAT_INPUT_BOUND
            }
        && b.output_token_bound == output
        && b.input_price_per_million > 0
        && if embedding {
            b.output_price_per_million == 0
        } else {
            b.output_price_per_million > 0
        }
        && [&b.configuration_version, &b.price_version]
            .iter()
            .all(|s| !s.is_empty() && s.len() <= 128 && s.bytes().all(|c| c.is_ascii_graphic()))
}
fn active(b: &ModelCallBudget) -> bool {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .is_some_and(|now| now < b.valid_until_unix_ms)
}
impl OpenAiAgentModels {
    /// 官方 HTTPS 固定端点；禁用环境代理、重定向及 HTTP 重试。
    /// # Errors
    /// 拒绝不匹配的模型、计数版本、费用上界、价格或密钥。
    pub fn new(
        key: &str,
        planning: ModelCallBudget,
        execution: ModelExecutionConfiguration,
    ) -> Result<Self, ReplySendError> {
        Self::build(
            key,
            planning,
            execution,
            Url::parse("https://api.openai.com/v1/").map_err(|_| invalid())?,
            Duration::from_secs(30),
        )
    }
    fn build(
        key: &str,
        planning: ModelCallBudget,
        execution: ModelExecutionConfiguration,
        base: Url,
        timeout: Duration,
    ) -> Result<Self, ReplySendError> {
        if key.trim().is_empty()
            || !valid_budget(&planning, false, 2048)
            || !valid_budget(&execution.answer, false, 1024)
            || !valid_budget(&execution.embedding, true, 0)
            || execution.version != execution.answer.configuration_version
            || ![&planning, &execution.answer, &execution.embedding]
                .iter()
                .all(|b| active(b))
        {
            return Err(invalid());
        }
        let mut headers = HeaderMap::new();
        let mut auth = HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| invalid())?;
        auth.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth);
        let client = Client::builder()
            .default_headers(headers)
            .no_proxy()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(timeout)
            .build()
            .map_err(|_| invalid())?;
        Ok(Self {
            client,
            base,
            planning,
            execution,
            chat_tokens: tiktoken_rs::o200k_base().map_err(|_| invalid())?,
            embedding_tokens: tiktoken_rs::cl100k_base().map_err(|_| invalid())?,
            disabled: AtomicBool::new(false),
        })
    }
    fn check(
        &self,
        expected: &ModelCallBudget,
        supplied: &ModelCallBudget,
    ) -> Result<(), ReplySendError> {
        if self.disabled.load(Ordering::SeqCst) || expected != supplied || !active(expected) {
            return Err(invalid());
        }
        Ok(())
    }
    async fn post(&self, path: &str, body: &Value) -> Result<Vec<u8>, ReplySendError> {
        let mut response = self
            .client
            .post(self.base.join(path).map_err(|_| invalid())?)
            .json(body)
            .send()
            .await
            .map_err(|_| ReplySendError::Unknown)?;
        if !response.status().is_success() {
            return Err(ReplySendError::Unknown);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ReplySendError::Unknown)?
        {
            if bytes.len() + chunk.len() > 128 * 1024 {
                return Err(ReplySendError::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    fn observe<T>(&self, result: Result<T, ReplySendError>) -> Result<T, ReplySendError> {
        if matches!(&result, Err(ReplySendError::ContractViolation)) {
            self.disabled.store(true, Ordering::SeqCst);
        }
        result
    }
    fn chat_payload(
        &self,
        stage: ModelChatStage,
        request: &ChatRequest,
        budget: &ModelCallBudget,
    ) -> Result<Value, ReplySendError> {
        let expected = match stage {
            ModelChatStage::Planning => &self.planning,
            ModelChatStage::Answer => &self.execution.answer,
        };
        self.check(expected, budget)?;
        if request.max_output_tokens.map(u64::from) != Some(budget.output_token_bound)
            || request.temperature.is_some()
            || !(2..=18).contains(&request.messages.len())
        {
            return Err(invalid());
        }
        let mut messages = Vec::new();
        let mut bytes = 0;
        let mut tokens = 0;
        for (index, message) in request.messages.iter().enumerate() {
            let role = if index == 0 { Role::System } else { Role::User };
            if message.role != role
                || message.content.trim().is_empty()
                || message.content.contains('\0')
            {
                return Err(invalid());
            }
            bytes += message.content.len();
            tokens += self.chat_tokens.encode_ordinary(&message.content).len();
            messages.push(
                json!({"role":if index == 0 {"system"} else {"user"},"content":message.content}),
            );
        }
        let schema = if stage == ModelChatStage::Planning {
            model_search_schema()
        } else {
            json!({"type":"object","additionalProperties":false,"required":["insufficient_evidence","answer","citations"],"properties":{"insufficient_evidence":{"type":"boolean"},"answer":{"type":"string"},"citations":{"type":"array","items":{"type":"integer"}}}})
        };
        tokens += self.chat_tokens.encode_ordinary(&schema.to_string()).len();
        // 本地正文计数只用于拒绝超限输入，完整上下文窗口仍作为计费预留。
        if bytes > 49_200 || tokens > 110_000 {
            return Err(invalid());
        }
        Ok(
            json!({"model":REPLY_MODEL,"messages":messages,"max_completion_tokens":budget.output_token_bound,"n":1,"stream":false,"store":false,"service_tier":"default","response_format":{"type":"json_schema","json_schema":{"name":"knowledge_agent","strict":true,"schema":schema}}}),
        )
    }
}
impl ModelAgentProvider for OpenAiAgentModels {
    fn chat<'a>(
        &'a self,
        stage: ModelChatStage,
        request: &'a ChatRequest,
        budget: &'a ModelCallBudget,
    ) -> BoxFuture<'a, Result<ReplyCompletion, ReplySendError>> {
        Box::pin(async move {
            let body = self.chat_payload(stage, request, budget)?;
            let bytes = self.post("chat/completions", &body).await?;
            self.observe(decode(
                &bytes,
                &ReplyBudget {
                    currency: budget.currency.clone(),
                    provider: budget.provider.clone(),
                    price_version: budget.price_version.clone(),
                    counter_version: budget.counter_version.clone(),
                    input_price_per_million: budget.input_price_per_million,
                    output_price_per_million: budget.output_price_per_million,
                    input_token_bound: budget.input_token_bound,
                    output_token_bound: budget.output_token_bound,
                    request_limit: i64::MAX,
                    daily_limit: i64::MAX,
                },
            ))
        })
    }
    fn embed<'a>(
        &'a self,
        query: &'a str,
        budget: &'a ModelCallBudget,
    ) -> BoxFuture<'a, Result<ModelEmbeddingCompletion, ReplySendError>> {
        Box::pin(async move {
            self.check(&self.execution.embedding, budget)?;
            if query.trim().is_empty()
                || query.contains('\0')
                || query.chars().count() > 1000
                || self.embedding_tokens.encode_ordinary(query).len() >= 8192
            {
                return Err(invalid());
            }
            let bytes = self.post("embeddings", &json!({"model":AGENT_EMBEDDING_MODEL,"input":[query],"dimensions":AGENT_EMBEDDING_DIMENSIONS,"encoding_format":"float"})).await?;
            self.observe(decode_embedding(&bytes, budget))
        })
    }
}
#[derive(Deserialize)]
struct EmbeddingResponse {
    model: String,
    data: Vec<EmbeddingItem>,
    usage: Option<Value>,
}
#[derive(Deserialize)]
struct EmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}
fn decode_embedding(
    bytes: &[u8],
    budget: &ModelCallBudget,
) -> Result<ModelEmbeddingCompletion, ReplySendError> {
    let response: EmbeddingResponse =
        serde_json::from_slice(bytes).map_err(|_| ReplySendError::InvalidResponse)?;
    if response.model != budget.model {
        return Err(ReplySendError::ContractViolation);
    }
    let usage = response.usage.as_ref();
    let input = usage
        .and_then(|v| v.get("prompt_tokens"))
        .and_then(Value::as_u64);
    let total = usage
        .and_then(|v| v.get("total_tokens"))
        .and_then(Value::as_u64);
    if input.is_some_and(|n| n > budget.input_token_bound)
        || total.is_some_and(|n| n > budget.input_token_bound)
    {
        return Err(ReplySendError::ContractViolation);
    }
    let trusted = usage
        .and_then(Value::as_object)
        .is_some_and(|v| v.len() == 2)
        && input.is_some_and(|n| n > 0)
        && input == total;
    let [item] = response.data.as_slice() else {
        return Err(ReplySendError::InvalidResponse);
    };
    if item.index != 0
        || item.embedding.len() != AGENT_EMBEDDING_DIMENSIONS
        || item.embedding.iter().any(|n| !n.is_finite())
        || !item.embedding.iter().any(|n| *n != 0.0)
    {
        return Err(ReplySendError::InvalidResponse);
    }
    Ok(ModelEmbeddingCompletion {
        embedding: Embedding {
            model: response.model,
            values: item.embedding.clone(),
        },
        usage: if trusted {
            input.map(|n| ReplyUsage {
                input_tokens: n,
                output_tokens: 0,
            })
        } else {
            None
        },
    })
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
