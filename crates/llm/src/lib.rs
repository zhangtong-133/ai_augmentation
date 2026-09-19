#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt::{self, Display};
use std::future::Future;
use std::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type LlmResult<T> = Result<T, LlmError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChatResponse {
    pub content: String,
    pub model: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Embedding {
    pub values: Vec<f32>,
    pub model: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LlmError {
    InvalidRequest(String),
    RateLimited,
    ProviderUnavailable(String),
    InvalidResponse(String),
}

impl Display for LlmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(formatter, "invalid LLM request: {message}"),
            Self::RateLimited => formatter.write_str("LLM provider rate limited the request"),
            Self::ProviderUnavailable(message) => {
                write!(formatter, "LLM provider unavailable: {message}")
            }
            Self::InvalidResponse(message) => write!(formatter, "invalid LLM response: {message}"),
        }
    }
}

impl Error for LlmError {}

pub trait LlmProvider: Send + Sync {
    fn chat(&self, request: &ChatRequest) -> BoxFuture<'_, LlmResult<ChatResponse>>;
    fn embedding(&self, input: &[String]) -> BoxFuture<'_, LlmResult<Vec<Embedding>>>;

    fn supports_streaming(&self) -> bool {
        false
    }
}

/// 独立的向量化端口，索引流程无需依赖聊天能力。
pub trait EmbeddingProvider: Send + Sync {
    fn embedding(&self, input: &[String]) -> BoxFuture<'_, LlmResult<Vec<Embedding>>>;
}
/// 仅使用给定证据回答问题，不提供工具执行能力。
#[derive(Clone, Debug)]
pub struct AnswerSource {
    pub id: usize,
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct ModelAnswer {
    pub answer: String,
    pub citations: Vec<usize>,
    pub insufficient_evidence: bool,
}
pub trait AnswerProvider: Send + Sync {
    fn answer(
        &self,
        question: &str,
        sources: &[AnswerSource],
    ) -> BoxFuture<'_, LlmResult<ModelAnswer>>;
}
