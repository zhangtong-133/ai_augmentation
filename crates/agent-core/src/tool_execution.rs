//! 显式工具调用共用的预算和审计入口；调用身份由应用提供，不自动规划或重试。
use personal_ai_storage::{
    StorageError,
    tool_calls::{
        NewToolCall, ToolCall, ToolCallFinish, ToolCallOutcome, ToolCallStart, ToolCallStore,
    },
};
use personal_ai_tools::{
    ExecutionError, ToolContext, ToolError, ToolExecutor, ToolRequest, ToolResponse,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Debug)]
pub enum AuditedToolError {
    Storage(StorageError),
    Execution(ExecutionError),
    AlreadyUsed(Box<ToolCall>),
}

pub struct AuditedToolResponse {
    pub call: ToolCall,
    pub response: ToolResponse,
}

pub struct AuditedToolExecutor {
    store: Arc<dyn ToolCallStore>,
    executor: Arc<ToolExecutor>,
}

impl AuditedToolExecutor {
    #[must_use]
    pub fn new(store: Arc<dyn ToolCallStore>, executor: Arc<ToolExecutor>) -> Self {
        Self { store, executor }
    }

    /// 验证白名单和参数，持久化次数后至多执行一次，再保存脱敏结果。
    ///
    /// # Errors
    /// 参数/工具无效、额度不足、重复 ID、存储失败或工具失败时返回错误。
    /// 持久化失败会阻止执行；完成后存储失败不自动重试或退还次数。
    pub async fn execute(
        &self,
        request_id: &str,
        name: &str,
        context: &ToolContext,
        request: &ToolRequest,
    ) -> Result<AuditedToolResponse, AuditedToolError> {
        self.executor
            .validate(name, request)
            .map_err(AuditedToolError::Execution)?;
        let call =
            prepare_tool_call(request_id, name, request).map_err(AuditedToolError::Execution)?;
        match self
            .store
            .start_tool_call(&context.user_id, &call)
            .await
            .map_err(AuditedToolError::Storage)?
        {
            ToolCallStart::Started(_) => {}
            ToolCallStart::Existing(call) => {
                return Err(AuditedToolError::AlreadyUsed(Box::new(call)));
            }
        }
        self.execute_registered(request_id, name, context, request)
            .await
    }

    // 只供本 crate 中已由仓储原子领取的计划步骤使用，不暴露为 HTTP 操作。
    pub(crate) async fn execute_registered(
        &self,
        request_id: &str,
        name: &str,
        context: &ToolContext,
        request: &ToolRequest,
    ) -> Result<AuditedToolResponse, AuditedToolError> {
        let result = self
            .executor
            .execute(name, context, request)
            .await
            .and_then(|response| {
                if response.is_error {
                    Err(ExecutionError::Tool(ToolError::ExecutionFailed(
                        "tool failed".into(),
                    )))
                } else {
                    Ok(response)
                }
            });
        let finish = match &result {
            Ok(response) => ToolCallFinish {
                outcome: ToolCallOutcome::Succeeded,
                output_bytes: i32::try_from(response.content.len()).ok(),
            },
            Err(error) => ToolCallFinish {
                outcome: outcome(error),
                output_bytes: None,
            },
        };
        let call = self
            .store
            .finish_tool_call(&context.user_id, request_id, finish)
            .await
            .map_err(AuditedToolError::Storage)?;
        result
            .map(|response| AuditedToolResponse { call, response })
            .map_err(AuditedToolError::Execution)
    }
}

/// 生成元数据审计参数；JSON 键顺序和空白不改变指纹。
/// # Errors
/// 参数不是有效 JSON 或超过执行器参数上限。
pub fn prepare_tool_call(
    request_id: &str,
    name: &str,
    request: &ToolRequest,
) -> Result<NewToolCall, ExecutionError> {
    let value: serde_json::Value = serde_json::from_str(&request.arguments_json)
        .map_err(|_| ExecutionError::InvalidArguments)?;
    let canonical = value.to_string();
    if !value.is_object() || canonical.len() > 8192 {
        return Err(ExecutionError::InvalidArguments);
    }
    Ok(NewToolCall {
        request_id: request_id.to_owned(),
        tool: name.to_owned(),
        arguments_digest: format!("{:x}", Sha256::digest(canonical.as_bytes())),
        input_bytes: i32::try_from(canonical.len())
            .map_err(|_| ExecutionError::InvalidArguments)?,
    })
}

#[cfg(test)]
#[path = "tool_execution_tests.rs"]
mod tests;

fn outcome(error: &ExecutionError) -> ToolCallOutcome {
    match error {
        ExecutionError::Busy => ToolCallOutcome::Busy,
        ExecutionError::Timeout => ToolCallOutcome::TimedOut,
        ExecutionError::OutputTooLarge => ToolCallOutcome::OutputRejected,
        ExecutionError::Tool(ToolError::PermissionDenied(_)) => ToolCallOutcome::Denied,
        // indexing_busy 也是先占用尝试次数后才发现的依赖并发限制。
        ExecutionError::Tool(ToolError::ExecutionFailed(code)) if code == "indexing_busy" => {
            ToolCallOutcome::Busy
        }
        _ => ToolCallOutcome::Failed,
    }
}
