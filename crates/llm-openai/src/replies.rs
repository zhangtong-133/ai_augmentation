//! 固定模型的离线计数与单次 HTTP 发送；不提供持久化领取或付费启用入口。
use personal_ai_agent_core::{
    budget::{CostReservation, TokenPrices},
    reply::{MAX_CONTEXT_BYTES, MAX_CONTEXT_MESSAGES, MAX_OUTPUT_TOKENS},
};
use personal_ai_storage::{
    StorageError, StorageResult,
    replies::{ReplyConfiguration, ReplyContext},
    reply_budgets::{ReplyBudget, ReplyBudgetPlanner, ReplyUsage},
};
use reqwest::{
    Client, Url,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub const REPLY_MODEL: &str = "gpt-4o-mini-2024-07-18";
pub const REPLY_COUNTER_VERSION: &str = "o200k-0.12.1-context-bound-v1";
const INPUT_BOUND: u64 = 128_000;
const ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";
const MAX_RESPONSE_BYTES: usize = 128 * 1024;

/// 部署者显式提供的 USD 价格和上限；没有内置市场报价。
#[derive(Clone)]
pub struct ReplyPrices {
    pub version: String,
    pub input_per_million: u64,
    pub output_per_million: u64,
    pub request_limit: i64,
    pub daily_limit: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplyTokenCount {
    /// 仅消息正文的本地 BPE token 数，不含服务端消息封装，不是计费依据。
    pub text_tokens: u64,
    /// 固定模型的完整上下文窗口，用作保守预留，不能用 `text_tokens` 替代。
    pub input_token_bound: u64,
}

pub struct OpenAiReplyPolicy {
    configuration: ReplyConfiguration,
    prices: ReplyPrices,
    tokenizer: tiktoken_rs::CoreBPE,
    disabled: AtomicBool,
}
fn invalid() -> StorageError {
    StorageError::InvalidData("invalid or disabled reply provider configuration".into())
}
fn valid_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.contains('\0')
}
impl OpenAiReplyPolicy {
    /// 固定模型快照，只接受显式价格及非敏感配置版本。初始化本地词表，不联网。
    /// # Errors
    /// 拒绝别名、未知模型、空版本、无效价格/额度或词表加载失败。
    pub fn new(configuration: ReplyConfiguration, prices: ReplyPrices) -> StorageResult<Self> {
        if configuration.model != REPLY_MODEL
            || !valid_id(&configuration.revision)
            || !valid_id(&prices.version)
            || prices.daily_limit <= 0
        {
            return Err(invalid());
        }
        CostReservation::quote(
            TokenPrices {
                input_per_million: prices.input_per_million,
                output_per_million: prices.output_per_million,
            },
            INPUT_BOUND,
            u64::from(MAX_OUTPUT_TOKENS),
            prices.request_limit,
        )
        .map_err(|_| invalid())?;
        Ok(Self {
            configuration,
            prices,
            tokenizer: tiktoken_rs::o200k_base().map_err(|_| invalid())?,
            disabled: AtomicBool::new(false),
        })
    }

    /// 停用当前配置；价格失效或供应商契约异常后，必须创建并审核新配置。
    pub fn disable(&self) {
        self.disabled.store(true, Ordering::SeqCst);
    }

    /// 核验冻结上下文并统计正文 BPE。特殊 token 拼写始终作为普通文本处理。
    /// # Errors
    /// 拒绝已停用、配置不匹配、超限或非法上下文。
    pub fn count(&self, context: &ReplyContext) -> StorageResult<ReplyTokenCount> {
        if self.disabled.load(Ordering::SeqCst)
            || context.configuration != self.configuration
            || context.max_output_tokens != MAX_OUTPUT_TOKENS
            || context.first_sequence < 1
            || context.user_messages.is_empty()
            || context.user_messages.len() > MAX_CONTEXT_MESSAGES
            || context.first_sequence
                > 101 - i64::try_from(context.user_messages.len()).map_err(|_| invalid())?
        {
            return Err(invalid());
        }
        let mut bytes = 0usize;
        let mut tokens = 0u64;
        for (index, text) in std::iter::once(&context.system)
            .chain(&context.user_messages)
            .enumerate()
        {
            if text.trim().is_empty()
                || text.contains('\0')
                || text.len() > MAX_CONTEXT_BYTES
                || (index > 0 && text.len() > 4096)
            {
                return Err(invalid());
            }
            bytes += text.len();
            if bytes > MAX_CONTEXT_BYTES {
                return Err(invalid());
            }
            tokens +=
                u64::try_from(self.tokenizer.encode_ordinary(text).len()).map_err(|_| invalid())?;
        }
        Ok(ReplyTokenCount {
            text_tokens: tokens,
            input_token_bound: INPUT_BOUND,
        })
    }
}
impl ReplyBudgetPlanner for OpenAiReplyPolicy {
    fn plan(&self, context: &ReplyContext) -> StorageResult<ReplyBudget> {
        let count = self.count(context)?;
        Ok(ReplyBudget {
            currency: "USD".into(),
            provider: "openai".into(),
            price_version: self.prices.version.clone(),
            counter_version: REPLY_COUNTER_VERSION.into(),
            input_price_per_million: self.prices.input_per_million,
            output_price_per_million: self.prices.output_per_million,
            input_token_bound: count.input_token_bound,
            output_token_bound: u64::from(context.max_output_tokens),
            request_limit: self.prices.request_limit,
            daily_limit: self.prices.daily_limit,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplySendError {
    /// 尚未发送：配置已停用、不匹配或请求/预算无效。
    InvalidConfiguration,
    /// 发送后结果不确定；不能推断未计费，禁止自动重发。
    Unknown,
    /// 响应不能作为有效回复或可信结算依据，保留全部预留。
    InvalidResponse,
    /// 模型/服务等级/费用上界契约异常，当前 policy 已停用。
    ContractViolation,
}
#[derive(Debug, PartialEq, Eq)]
pub struct ReplyCompletion {
    pub content: String,
    /// 缺失或不完整的 usage 不允许退差额。
    pub usage: Option<ReplyUsage>,
}

pub struct OpenAiReplies {
    client: Client,
    endpoint: Url,
    policy: Arc<OpenAiReplyPolicy>,
}
impl OpenAiReplies {
    /// 只连接官方 HTTPS 端点，禁用环境代理、重定向及所有 reqwest 自动重试。
    /// # Errors
    /// 凭据为空、非法请求头或 HTTP 客户端初始化失败时拒绝创建。
    pub fn new(key: &str, policy: Arc<OpenAiReplyPolicy>) -> Result<Self, ReplySendError> {
        Self::build(
            key,
            policy,
            Url::parse(ENDPOINT).map_err(|_| ReplySendError::InvalidConfiguration)?,
            Duration::from_secs(30),
        )
    }
    fn build(
        key: &str,
        policy: Arc<OpenAiReplyPolicy>,
        endpoint: Url,
        timeout: Duration,
    ) -> Result<Self, ReplySendError> {
        if key.trim().is_empty() {
            return Err(ReplySendError::InvalidConfiguration);
        }
        let mut headers = HeaderMap::new();
        let mut token = HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| ReplySendError::InvalidConfiguration)?;
        token.set_sensitive(true);
        headers.insert(AUTHORIZATION, token);
        let client = Client::builder()
            .default_headers(headers)
            .no_proxy()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(timeout)
            .build()
            .map_err(|_| ReplySendError::InvalidConfiguration)?;
        Ok(Self {
            client,
            endpoint,
            policy,
        })
    }
    /// 每次调用最多发出一次 HTTP 请求。调用者必须先确认数据库领取事务已提交，
    /// 且 budget 来自该请求的持久化凭据；此方法本身不提供跨调用幂等。
    /// # Errors
    /// 无效配置在发送前拒绝；发送后的失败一律不得自动重试或退款。
    pub async fn send_once(
        &self,
        context: &ReplyContext,
        budget: &ReplyBudget,
    ) -> Result<ReplyCompletion, ReplySendError> {
        let expected = self
            .policy
            .plan(context)
            .map_err(|_| ReplySendError::InvalidConfiguration)?;
        if expected != *budget {
            return Err(ReplySendError::InvalidConfiguration);
        }
        let messages: Vec<_> = std::iter::once(json!({"role":"system","content":context.system}))
            .chain(
                context
                    .user_messages
                    .iter()
                    .map(|text| json!({"role":"user","content":text})),
            )
            .collect();
        let payload = json!({"model": REPLY_MODEL, "messages":messages, "max_completion_tokens":context.max_output_tokens,
            "n":1, "stream":false, "store":false, "service_tier":"default"});
        let mut response = self
            .client
            .post(self.endpoint.clone())
            .json(&payload)
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
            if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(ReplySendError::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        let result = decode(&bytes, budget);
        if result == Err(ReplySendError::ContractViolation) {
            self.policy.disable();
        }
        result
    }
}

#[derive(Deserialize)]
struct Completion {
    model: String,
    service_tier: String,
    choices: Vec<Choice>,
    usage: Option<Value>,
}
#[derive(Deserialize)]
struct Choice {
    index: u32,
    finish_reason: String,
    message: Message,
}
#[derive(Deserialize)]
struct Message {
    role: String,
    content: Option<String>,
    refusal: Option<Value>,
    tool_calls: Option<Value>,
    function_call: Option<Value>,
    audio: Option<Value>,
}
fn decode(bytes: &[u8], budget: &ReplyBudget) -> Result<ReplyCompletion, ReplySendError> {
    let response: Completion =
        serde_json::from_slice(bytes).map_err(|_| ReplySendError::InvalidResponse)?;
    if response.model != REPLY_MODEL || response.service_tier != "default" {
        return Err(ReplySendError::ContractViolation);
    }
    let usage = decode_usage(response.usage.as_ref(), budget)?;
    let [choice] = response.choices.as_slice() else {
        return Err(ReplySendError::InvalidResponse);
    };
    if choice.index != 0
        || choice.finish_reason != "stop"
        || choice.message.role != "assistant"
        || choice.message.refusal.is_some()
        || choice.message.tool_calls.is_some()
        || choice.message.function_call.is_some()
        || choice.message.audio.is_some()
    {
        return Err(ReplySendError::InvalidResponse);
    }
    let content = choice
        .message
        .content
        .as_deref()
        .ok_or(ReplySendError::InvalidResponse)?;
    if content.trim().is_empty() || content.len() > 16384 || content.contains('\0') {
        return Err(ReplySendError::InvalidResponse);
    }
    Ok(ReplyCompletion {
        content: content.into(),
        usage,
    })
}
fn decode_usage(
    value: Option<&Value>,
    budget: &ReplyBudget,
) -> Result<Option<ReplyUsage>, ReplySendError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let input = value.get("prompt_tokens").and_then(Value::as_u64);
    let output = value.get("completion_tokens").and_then(Value::as_u64);
    // 即便其余字段不完整，已观察到的越界仍停用配置。
    if input.is_some_and(|n| n > budget.input_token_bound)
        || output.is_some_and(|n| n > budget.output_token_bound)
    {
        return Err(ReplySendError::ContractViolation);
    }
    let Some(fields) = value.as_object() else {
        return Ok(None);
    };
    if fields.keys().any(|key| {
        ![
            "prompt_tokens",
            "completion_tokens",
            "total_tokens",
            "prompt_tokens_details",
            "completion_tokens_details",
        ]
        .contains(&key.as_str())
    }) {
        return Ok(None);
    }
    let (Some(input), Some(output), Some(total)) = (
        input,
        output,
        value.get("total_tokens").and_then(Value::as_u64),
    ) else {
        return Ok(None);
    };
    if input == 0 || output == 0 || input.checked_add(output) != Some(total) {
        return Ok(None);
    }
    for (field, allowed) in [
        (
            "prompt_tokens_details",
            &["cached_tokens", "audio_tokens"][..],
        ),
        (
            "completion_tokens_details",
            &[
                "reasoning_tokens",
                "audio_tokens",
                "accepted_prediction_tokens",
                "rejected_prediction_tokens",
            ][..],
        ),
    ] {
        if let Some(details) = value.get(field).filter(|v| !v.is_null()) {
            let Some(details) = details.as_object() else {
                return Ok(None);
            };
            for (key, number) in details {
                let Some(number) = number.as_u64() else {
                    return Ok(None);
                };
                if !allowed.contains(&key.as_str()) {
                    return Ok(None);
                }
                if key == "cached_tokens" {
                    if number > input {
                        return Ok(None);
                    }
                } else if number != 0 {
                    return Err(ReplySendError::ContractViolation);
                }
            }
        }
    }
    Ok(Some(ReplyUsage {
        input_tokens: input,
        output_tokens: output,
    }))
}

#[cfg(test)]
#[path = "reply_tests.rs"]
mod tests;
