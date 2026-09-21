use crate::{ApiError, AppState, Indexing, auth};
use axum::{
    Extension, Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use personal_ai_domain::ConversationId;
use personal_ai_storage::documents::DocumentStore;
use personal_ai_tools::{
    BoxFuture, ExecutionError, Tool, ToolContext, ToolError, ToolExecutor, ToolRequest,
    ToolResponse,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

struct KnowledgeSearch {
    indexing: Arc<Indexing>,
    documents: Arc<dyn DocumentStore>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    5
}

impl Tool for KnowledgeSearch {
    fn name(&self) -> &'static str {
        "knowledge_search"
    }
    fn description(&self) -> &'static str {
        "检索当前用户已索引的知识片段；只读，但会调用向量模型并可能计费。"
    }
    fn input_schema_json(&self) -> &'static str {
        r#"{"type":"object","properties":{"query":{"type":"string","minLength":1,"maxLength":1000},"limit":{"type":"integer","minimum":1,"maximum":5,"default":5}},"required":["query"],"additionalProperties":false}"#
    }
    fn execute(
        &self,
        context: &ToolContext,
        request: &ToolRequest,
    ) -> BoxFuture<'_, Result<ToolResponse, ToolError>> {
        let arguments = serde_json::from_str::<Arguments>(&request.arguments_json);
        let owner = context.user_id.clone();
        Box::pin(async move {
            let input = arguments
                .map_err(|_| ToolError::InvalidArguments("invalid search arguments".into()))?;
            if input.query.trim().is_empty()
                || input.query.chars().count() > 1000
                || !(1..=5).contains(&input.limit)
            {
                return Err(ToolError::InvalidArguments(
                    "invalid search arguments".into(),
                ));
            }
            let _permit = self
                .indexing
                .slots
                .try_acquire()
                .map_err(|_| ToolError::ExecutionFailed("indexing_busy".into()))?;
            let hits = self
                .indexing
                .indexer
                .search(&owner, self.documents.as_ref(), &input.query, input.limit)
                .await
                .map_err(|_| ToolError::ExecutionFailed("search unavailable".into()))?;
            Ok(ToolResponse {
                content: json!({"hits": hits}).to_string(),
                is_error: false,
            })
        })
    }
}

pub(super) fn routes(state: &AppState) -> Router<AppState> {
    let tools: Vec<Arc<dyn Tool>> = state
        .indexing
        .as_ref()
        .map(|indexing| {
            vec![Arc::new(KnowledgeSearch {
                indexing: indexing.clone(),
                documents: state.documents.clone(),
            }) as Arc<dyn Tool>]
        })
        .unwrap_or_default();
    let executor = Arc::new(ToolExecutor::new(tools).expect("static unique tool registry"));
    Router::new()
        .route("/api/tools", get(list))
        .route("/api/tools/{name}", post(execute))
        .layer(Extension(executor))
}

async fn list(
    State(state): State<AppState>,
    Extension(executor): Extension<Arc<ToolExecutor>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    auth::current_user(&state, &headers).await?;
    let tools: Vec<_> = executor.tools().map(|tool| json!({
        "name": tool.name(), "description": tool.description(),
        "input_schema": serde_json::from_str::<Value>(tool.input_schema_json()).unwrap_or(Value::Null),
        "read_only": true, "may_incur_cost": true,
    })).collect();
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"tools":tools})),
    ))
}

async fn execute(
    State(state): State<AppState>,
    Extension(executor): Extension<Arc<ToolExecutor>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let Json(arguments) = payload.map_err(|error| ApiError(error.status(), "invalid_json"))?;
    // 身份和调用 ID 只由服务端构造，客户端不能指定所有者或借用其他会话。
    let context = ToolContext {
        user_id: user.id,
        conversation_id: ConversationId::new(Uuid::new_v4().to_string()),
    };
    let result = executor
        .execute(
            &name,
            &context,
            &ToolRequest {
                arguments_json: arguments.to_string(),
            },
        )
        .await
        .map_err(|error| match error {
            ExecutionError::UnknownTool => ApiError(StatusCode::NOT_FOUND, "tool_not_available"),
            ExecutionError::InvalidArguments
            | ExecutionError::Tool(ToolError::InvalidArguments(_)) => {
                ApiError(StatusCode::BAD_REQUEST, "invalid_tool_arguments")
            }
            ExecutionError::Busy => ApiError(StatusCode::TOO_MANY_REQUESTS, "tool_busy"),
            ExecutionError::Timeout => ApiError(StatusCode::GATEWAY_TIMEOUT, "tool_timeout"),
            ExecutionError::Tool(ToolError::PermissionDenied(_)) => {
                ApiError(StatusCode::FORBIDDEN, "tool_denied")
            }
            ExecutionError::Tool(ToolError::ExecutionFailed(code)) if code == "indexing_busy" => {
                ApiError(StatusCode::TOO_MANY_REQUESTS, "tool_busy")
            }
            _ => ApiError(StatusCode::BAD_GATEWAY, "tool_failed"),
        })?;
    if result.is_error {
        return Err(ApiError(StatusCode::BAD_GATEWAY, "tool_failed"));
    }
    let output: Value = serde_json::from_str(&result.content)
        .map_err(|_| ApiError(StatusCode::BAD_GATEWAY, "tool_failed"))?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"tool": name, "output": output})),
    ))
}
