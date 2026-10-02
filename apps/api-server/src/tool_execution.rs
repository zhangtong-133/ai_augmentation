use crate::{ApiError, AppState, Indexing, auth};
use axum::{
    Extension, Json, Router,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use personal_ai_agent_core::tool_execution::{AuditedToolError, AuditedToolExecutor};
use personal_ai_domain::ConversationId;
use personal_ai_storage::agent_plans::KnowledgeQuery;
use personal_ai_storage::documents::DocumentStore;
use personal_ai_storage::{StorageError, tool_calls::DAILY_TOOL_CALL_LIMIT};
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

fn arguments(request: &ToolRequest) -> Result<KnowledgeQuery, ToolError> {
    let input = serde_json::from_str::<KnowledgeQuery>(&request.arguments_json)
        .map_err(|_| ToolError::InvalidArguments("invalid search arguments".into()))?;
    input
        .validate()
        .map_err(|_| ToolError::InvalidArguments("invalid search arguments".into()))?;
    Ok(input)
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
    fn validate(&self, request: &ToolRequest) -> Result<(), ToolError> {
        arguments(request).map(|_| ())
    }
    fn execute(
        &self,
        context: &ToolContext,
        request: &ToolRequest,
    ) -> BoxFuture<'_, Result<ToolResponse, ToolError>> {
        let arguments = arguments(request);
        let owner = context.user_id.clone();
        Box::pin(async move {
            let input = arguments?;
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

pub(super) fn executor(state: &AppState) -> Arc<ToolExecutor> {
    let mut tools: Vec<Arc<dyn Tool>> = state
        .indexing
        .as_ref()
        .map(|indexing| {
            vec![Arc::new(KnowledgeSearch {
                indexing: indexing.clone(),
                documents: state.documents.clone(),
            }) as Arc<dyn Tool>]
        })
        .unwrap_or_default();
    if let Some(search) = &state.web_search {
        tools.push(search.clone());
    }
    if let Some(git) = &state.git_tool {
        tools.push(git.clone());
    }
    tools.push(Arc::new(crate::file_reader::FileReader(
        state.documents.clone(),
    )));
    Arc::new(ToolExecutor::new(tools).expect("static unique tool registry"))
}

pub(super) fn routes(executor: Arc<ToolExecutor>) -> Router<AppState> {
    Router::new()
        .route("/api/tools", get(list))
        .route("/api/tools/{name}", post(execute))
        .route("/api/tool-calls", get(audit))
        .route("/api/tool-calls/{id}", get(detail))
        .layer(Extension(executor))
}

async fn list(
    State(state): State<AppState>,
    Extension(executor): Extension<Arc<ToolExecutor>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    auth::current_user(&state, &headers).await?;
    Ok(manifest(&executor, None))
}

pub(super) fn manifest(executor: &ToolExecutor, scope: Option<&str>) -> impl IntoResponse + use<> {
    let tools: Vec<_> = executor.tools().filter(|tool| scope.is_none_or(|name| tool.name() == name)).map(|tool| json!({
        "name": tool.name(), "description": tool.description(),
        "input_schema": serde_json::from_str::<Value>(tool.input_schema_json()).unwrap_or(Value::Null),
        "read_only": true, "may_incur_cost": tool.may_incur_cost(),
        "request_id_header": "Idempotency-Key", "daily_call_limit": DAILY_TOOL_CALL_LIMIT,
    })).collect();
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"tools":tools})),
    )
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
    execute_for(state, executor, user.id, name, headers, payload).await
}

pub(super) async fn execute_for(
    state: AppState,
    executor: Arc<ToolExecutor>,
    owner: personal_ai_domain::UserId,
    name: String,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(arguments) = payload.map_err(|error| ApiError(error.status(), "invalid_json"))?;
    let request = ToolRequest {
        arguments_json: arguments.to_string(),
    };
    executor
        .validate(&name, &request)
        .map_err(execution_error)?;
    let id = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or(ApiError(StatusCode::BAD_REQUEST, "invalid_tool_call_id"))?;
    if headers.get_all("idempotency-key").iter().count() != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_tool_call_id"));
    }
    let store = state.tool_calls.ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "tool_audit_unavailable",
    ))?;
    // 身份和工具上下文由服务端构造，客户端不能指定所有者或借用其他会话。
    let context = ToolContext {
        user_id: owner,
        conversation_id: ConversationId::new(Uuid::new_v4().to_string()),
    };
    let result = AuditedToolExecutor::new(store, executor)
        .execute(&id.to_string(), &name, &context, &request)
        .await
        .map_err(|error| match error {
            AuditedToolError::Execution(error) => execution_error(error),
            AuditedToolError::Storage(error) => storage_error(error),
            AuditedToolError::AlreadyUsed(_) => {
                ApiError(StatusCode::CONFLICT, "tool_call_already_used")
            }
        })?;
    let output: Value = serde_json::from_str(&result.response.content)
        .map_err(|_| ApiError(StatusCode::BAD_GATEWAY, "tool_failed"))?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"tool": name, "output": output, "call": result.call})),
    ))
}

pub(super) fn execution_error(error: ExecutionError) -> ApiError {
    match error {
        ExecutionError::UnknownTool => ApiError(StatusCode::NOT_FOUND, "tool_not_available"),
        ExecutionError::InvalidArguments | ExecutionError::Tool(ToolError::InvalidArguments(_)) => {
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
    }
}

fn storage_error(error: StorageError) -> ApiError {
    match error {
        StorageError::Conflict(code) if code == "tool call quota reached" => {
            ApiError(StatusCode::TOO_MANY_REQUESTS, "tool_daily_limit")
        }
        StorageError::Conflict(_) => ApiError(StatusCode::CONFLICT, "tool_call_conflict"),
        StorageError::NotFound => ApiError(StatusCode::NOT_FOUND, "tool_call_not_found"),
        StorageError::InvalidData(_) => ApiError(StatusCode::BAD_REQUEST, "invalid_tool_call"),
        StorageError::Unavailable(_) => {
            ApiError(StatusCode::SERVICE_UNAVAILABLE, "tool_audit_unavailable")
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditQuery {
    day: Option<String>,
}

async fn audit(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<AuditQuery>, QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let Query(query) =
        query.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_tool_call_query"))?;
    let store = state.tool_calls.ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "tool_audit_unavailable",
    ))?;
    let report = store
        .audit_tool_calls(&user.id, query.day.as_deref())
        .await
        .map_err(storage_error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(json!(report))))
}

async fn detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let id = Uuid::parse_str(&id)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_tool_call_id"))?;
    let store = state.tool_calls.ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "tool_audit_unavailable",
    ))?;
    let call = store
        .get_tool_call(&user.id, &id.to_string())
        .await
        .map_err(storage_error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(json!(call))))
}
