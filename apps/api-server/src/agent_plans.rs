use crate::{ApiError, AppState, auth, tool_execution::execution_error};
use axum::{
    Extension, Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use personal_ai_agent_core::knowledge_plan::KnowledgePlanExecutor;
use personal_ai_storage::{
    StorageError,
    agent_plans::{
        AgentPlanApproval, AgentPlanStore, KnowledgeQuery, MAX_PLAN_TOOL_CALLS, NewAgentPlan,
        PLAN_VERSION,
    },
};
use personal_ai_tools::{ToolExecutor, ToolRequest};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    request_id: Uuid,
    expected_revision: i64,
    searches: Vec<KnowledgeQuery>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approve {
    plan_digest: String,
    accepted_call_limit: i32,
    acknowledge_embedding_cost: bool,
}
pub(super) fn routes(executor: Arc<ToolExecutor>) -> Router<AppState> {
    Router::new()
        .route(
            "/api/conversations/{id}/agent-plans",
            get(list).post(create),
        )
        .route("/api/conversations/{id}/agent-plans/{request}", get(read))
        .route(
            "/api/conversations/{id}/agent-plans/{request}/approve",
            post(approve),
        )
        .route(
            "/api/conversations/{id}/agent-plans/{request}/cancel",
            post(cancel),
        )
        .layer(Extension(executor))
}
fn key(value: &str) -> Result<String, ApiError> {
    Uuid::parse_str(value)
        .map(|value| value.to_string())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_agent_plan_id"))
}
fn store(state: &AppState) -> Result<Arc<dyn AgentPlanStore>, ApiError> {
    state.agent_plans.clone().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "agent_plans_unavailable",
    ))
}
fn storage_error(error: StorageError) -> ApiError {
    match error {
        StorageError::NotFound => ApiError(StatusCode::NOT_FOUND, "agent_plan_not_found"),
        StorageError::InvalidData(_) => ApiError(StatusCode::BAD_REQUEST, "invalid_agent_plan"),
        StorageError::Conflict(reason) if reason == "plan quota reached" => {
            ApiError(StatusCode::TOO_MANY_REQUESTS, "agent_plan_limit")
        }
        StorageError::Conflict(_) => ApiError(StatusCode::CONFLICT, "agent_plan_conflict"),
        StorageError::Unavailable(_) => {
            ApiError(StatusCode::SERVICE_UNAVAILABLE, "agent_plans_unavailable")
        }
    }
}
fn validate_tools(executor: &ToolExecutor, searches: &[KnowledgeQuery]) -> Result<(), ApiError> {
    for arguments in searches {
        executor
            .validate(
                "knowledge_search",
                &ToolRequest {
                    arguments_json: json!(arguments).to_string(),
                },
            )
            .map_err(execution_error)?;
    }
    Ok(())
}
async fn create(
    State(state): State<AppState>,
    Extension(executor): Extension<Arc<ToolExecutor>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    payload: Result<Json<Create>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let Json(input) = payload.map_err(|error| ApiError(error.status(), "invalid_agent_plan"))?;
    let input = NewAgentPlan {
        request_id: input.request_id.to_string(),
        expected_revision: input.expected_revision,
        searches: input.searches,
    };
    input.validate().map_err(storage_error)?;
    validate_tools(&executor, &input.searches)?;
    let plan = store(&state)?
        .create_agent_plan(&user.id, &key(&id)?, &input)
        .await
        .map_err(storage_error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(plan)))
}
async fn read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, request)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let plan = store(&state)?
        .get_agent_plan(&user.id, &key(&id)?, &key(&request)?)
        .await
        .map_err(storage_error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(plan)))
}
async fn list(
    State(state): State<AppState>,
    Extension(executor): Extension<Arc<ToolExecutor>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let plans = store(&state)?
        .list_agent_plans(&user.id, &key(&id)?)
        .await
        .map_err(storage_error)?;
    let enabled = executor
        .tools()
        .any(|tool| tool.name() == "knowledge_search");
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(
            json!({"enabled":enabled,"version":PLAN_VERSION,"max_tool_calls":MAX_PLAN_TOOL_CALLS,"plans":plans}),
        ),
    ))
}
async fn approve(
    State(state): State<AppState>,
    Extension(executor): Extension<Arc<ToolExecutor>>,
    headers: HeaderMap,
    Path((id, request)): Path<(String, String)>,
    payload: Result<Json<Approve>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let Json(input) = payload.map_err(|error| ApiError(error.status(), "invalid_agent_plan"))?;
    let (id, request) = (key(&id)?, key(&request)?);
    let store = store(&state)?;
    let preview = store
        .get_agent_plan(&user.id, &id, &request)
        .await
        .map_err(storage_error)?;
    validate_tools(
        &executor,
        &preview
            .steps
            .iter()
            .map(|step| step.arguments.clone())
            .collect::<Vec<_>>(),
    )?;
    let result = store
        .approve_agent_plan(
            &user.id,
            &id,
            &request,
            &AgentPlanApproval {
                plan_digest: input.plan_digest,
                accepted_call_limit: input.accepted_call_limit,
                acknowledge_embedding_cost: input.acknowledge_embedding_cost,
            },
        )
        .await
        .map_err(storage_error)?;
    if result.started {
        tokio::spawn(async move {
            KnowledgePlanExecutor::new(store, executor)
                .run(&user.id, &id, &request)
                .await;
        });
    }
    Ok((
        StatusCode::ACCEPTED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(result.plan),
    ))
}
async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, request)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let plan = store(&state)?
        .cancel_agent_plan(&user.id, &key(&id)?, &key(&request)?)
        .await
        .map_err(storage_error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(plan)))
}
