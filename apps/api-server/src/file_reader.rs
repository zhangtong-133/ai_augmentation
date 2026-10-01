//! 只读取当前用户已导入文档的文本投影，不接受文件路径或 URL。
use personal_ai_storage::{StorageError, documents::DocumentStore};
use personal_ai_tools::{BoxFuture, Tool, ToolContext, ToolError, ToolRequest, ToolResponse};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

pub(super) struct FileReader(pub Arc<dyn DocumentStore>);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    document_id: Uuid,
    #[serde(default)]
    offset: u32,
    #[serde(default = "default_limit")]
    limit: u32,
}
fn default_limit() -> u32 {
    2000
}
fn input(request: &ToolRequest) -> Result<Input, ToolError> {
    let value: Input = serde_json::from_str(&request.arguments_json).map_err(|_| invalid())?;
    if !(1..=4000).contains(&value.limit) || value.offset > 2_000_000 {
        return Err(invalid());
    }
    Ok(value)
}
fn invalid() -> ToolError {
    ToolError::InvalidArguments("invalid document range".into())
}

impl Tool for FileReader {
    fn name(&self) -> &'static str {
        "file_reader"
    }
    fn description(&self) -> &'static str {
        "分页读取当前用户已导入文档的文本；偏移按 Unicode 字符计数，不读取服务器文件或访问来源 URL，不调用模型。"
    }
    fn may_incur_cost(&self) -> bool {
        false
    }
    fn input_schema_json(&self) -> &'static str {
        r#"{"type":"object","properties":{"document_id":{"type":"string","format":"uuid"},"offset":{"type":"integer","minimum":0,"maximum":2000000,"default":0},"limit":{"type":"integer","minimum":1,"maximum":4000,"default":2000}},"required":["document_id"],"additionalProperties":false}"#
    }
    fn validate(&self, request: &ToolRequest) -> Result<(), ToolError> {
        input(request).map(|_| ())
    }
    fn execute(
        &self,
        context: &ToolContext,
        request: &ToolRequest,
    ) -> BoxFuture<'_, Result<ToolResponse, ToolError>> {
        let input = input(request);
        let owner = context.user_id.clone();
        Box::pin(async move {
            let input = input?;
            let document = self
                .0
                .get_document_text(&owner, &input.document_id.to_string())
                .await
                .map_err(|error| match error {
                    StorageError::NotFound => {
                        ToolError::PermissionDenied("document unavailable".into())
                    }
                    _ => ToolError::ExecutionFailed("document read unavailable".into()),
                })?;
            let total = document.markdown.chars().count();
            let offset = input.offset as usize;
            if offset > total {
                return Err(invalid());
            }
            let text: String = document
                .markdown
                .chars()
                .skip(offset)
                .take(input.limit as usize)
                .collect();
            let end = offset + text.chars().count();
            Ok(ToolResponse {
                content: json!({"document_id": input.document_id, "title": document.summary.title,
                    "source_type": document.summary.source_type, "text": text, "offset": input.offset,
                    "next_offset": (end < total).then_some(end), "total_chars": total}).to_string(),
                is_error: false,
            })
        })
    }
}
