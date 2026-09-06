#![forbid(unsafe_code)]

use personal_ai_domain::{ConversationId, UserId};
use std::error::Error;
use std::fmt::{self, Display};
use std::future::Future;
use std::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolContext {
    pub user_id: UserId,
    pub conversation_id: ConversationId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolRequest {
    pub arguments_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolResponse {
    pub content: String,
    pub is_error: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolError {
    InvalidArguments(String),
    PermissionDenied(String),
    ExecutionFailed(String),
}

impl Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArguments(message) => {
                write!(formatter, "invalid tool arguments: {message}")
            }
            Self::PermissionDenied(message) => {
                write!(formatter, "tool permission denied: {message}")
            }
            Self::ExecutionFailed(message) => write!(formatter, "tool execution failed: {message}"),
        }
    }
}

impl Error for ToolError {}

pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema_json(&self) -> &'static str;
    fn execute(
        &self,
        context: &ToolContext,
        request: &ToolRequest,
    ) -> BoxFuture<'_, Result<ToolResponse, ToolError>>;
}
