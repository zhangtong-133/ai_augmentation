#![forbid(unsafe_code)]

pub mod reply;

use personal_ai_domain::{ConversationId, UserId};
use personal_ai_llm::ChatMessage;
use std::error::Error;
use std::fmt::{self, Display};
use std::future::Future;
use std::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentInput {
    pub user_id: UserId,
    pub conversation_id: ConversationId,
    pub content: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentContext {
    pub messages: Vec<ChatMessage>,
    pub relevant_memory: Vec<String>,
    pub token_budget: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanStep {
    CallTool {
        name: String,
        arguments_json: String,
    },
    AskModel,
    PersistMemory {
        key: String,
        value: String,
    },
    Respond(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub steps: Vec<PlanStep>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentError {
    Context(String),
    Planning(String),
    Tool(String),
    Model(String),
    Memory(String),
}

impl Display for AgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Context(message) => write!(formatter, "context error: {message}"),
            Self::Planning(message) => write!(formatter, "planning error: {message}"),
            Self::Tool(message) => write!(formatter, "tool error: {message}"),
            Self::Model(message) => write!(formatter, "model error: {message}"),
            Self::Memory(message) => write!(formatter, "memory error: {message}"),
        }
    }
}

impl Error for AgentError {}

pub trait ContextBuilder: Send + Sync {
    fn build(&self, input: &AgentInput) -> BoxFuture<'_, Result<AgentContext, AgentError>>;
}

pub trait Planner: Send + Sync {
    fn plan(
        &self,
        input: &AgentInput,
        context: &AgentContext,
    ) -> BoxFuture<'_, Result<Plan, AgentError>>;
}

/// 描述 v1 编排阶段的标记。具体适配器由应用 crate 组装，
/// 不由本领域 crate 持有。
pub const V1_RUNTIME_STAGES: [&str; 6] = [
    "context_builder",
    "planner",
    "tool_executor",
    "llm",
    "memory_update",
    "response",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_order_keeps_memory_after_model_execution() {
        let model = V1_RUNTIME_STAGES
            .iter()
            .position(|stage| *stage == "llm")
            .unwrap();
        let memory = V1_RUNTIME_STAGES
            .iter()
            .position(|stage| *stage == "memory_update")
            .unwrap();

        assert!(model < memory);
    }
}
