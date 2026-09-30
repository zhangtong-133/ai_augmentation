//! 持久化工具执行次数与元数据；不保存参数或工具输出正文。
use crate::{BoxFuture, StorageError, StorageResult};
use personal_ai_domain::UserId;

pub const DAILY_TOOL_CALL_LIMIT: i32 = 100;

#[derive(Clone, Debug)]
pub struct NewToolCall {
    pub request_id: String,
    pub tool: String,
    pub arguments_digest: String,
    pub input_bytes: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ToolCall {
    pub request_id: String,
    pub tool: String,
    pub day: String,
    pub status: String,
    pub input_bytes: i32,
    pub output_bytes: Option<i32>,
    pub created_at_unix_ms: i64,
    pub deadline_unix_ms: i64,
    pub finished_at_unix_ms: Option<i64>,
}

#[derive(Debug)]
pub enum ToolCallStart {
    Started(ToolCall),
    Existing(ToolCall),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolCallOutcome {
    Succeeded,
    Failed,
    TimedOut,
    Busy,
    Denied,
    OutputRejected,
}

impl ToolCallOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Busy => "busy",
            Self::Denied => "denied",
            Self::OutputRejected => "output_rejected",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolCallFinish {
    pub outcome: ToolCallOutcome,
    pub output_bytes: Option<i32>,
}

#[derive(Debug, serde::Serialize)]
pub struct ToolCallAudit {
    pub day: String,
    pub used: i32,
    pub limit: i32,
    pub items: Vec<ToolCall>,
}

pub trait ToolCallStore: Send + Sync {
    /// 在执行之前持久化一次性调用和 UTC 日次数；同 ID 不再次占用次数。
    fn start_tool_call(
        &self,
        owner: &UserId,
        call: &NewToolCall,
    ) -> BoxFuture<'_, StorageResult<ToolCallStart>>;

    /// 仅写终态元数据，不退还次数；重复相同结算幂等，拒绝覆盖其他结算。
    fn finish_tool_call(
        &self,
        owner: &UserId,
        request_id: &str,
        finish: ToolCallFinish,
    ) -> BoxFuture<'_, StorageResult<ToolCall>>;

    fn get_tool_call(
        &self,
        owner: &UserId,
        request_id: &str,
    ) -> BoxFuture<'_, StorageResult<ToolCall>>;

    /// 同一只读快照读取次数和当日全部记录（最多 100 条）；None 使用数据库 UTC 今天。
    fn audit_tool_calls(
        &self,
        owner: &UserId,
        day: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ToolCallAudit>>;
}

/// # Errors
/// 拒绝无效工具名、参数指纹和超出执行器边界的输入大小。
pub fn validate_new_tool_call(call: &NewToolCall) -> StorageResult<()> {
    if call.tool.is_empty()
        || call.tool.len() > 64
        || !call
            .tool
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'_' || b.is_ascii_digit())
        || call.arguments_digest.len() != 64
        || !call
            .arguments_digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || !(2..=8192).contains(&call.input_bytes)
    {
        return Err(StorageError::InvalidData("invalid tool call".into()));
    }
    Ok(())
}

/// # Errors
/// 成功记录须包含有界输出大小，其他结果不得记录正文大小。
pub fn validate_tool_call_finish(finish: ToolCallFinish) -> StorageResult<()> {
    let valid = if finish.outcome == ToolCallOutcome::Succeeded {
        finish
            .output_bytes
            .is_some_and(|n| (0..=65536).contains(&n))
    } else {
        finish.output_bytes.is_none()
    };
    if !valid {
        return Err(StorageError::InvalidData(
            "invalid tool call outcome".into(),
        ));
    }
    Ok(())
}
