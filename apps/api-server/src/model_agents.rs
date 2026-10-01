//! 两阶段金额授权 HTTP；客户端不能指定模型、价格、查询或内部领取凭据。
use crate::{ApiError, AppState, auth};
use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use personal_ai_agent_core::model_executor::{ModelAgentExecutor, ModelAgentStore};
use personal_ai_storage::{
    StorageError,
    model_agents::{AgentCallCounts, AgentQuoteApproval, NewModelPlanningRequest},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

pub struct ModelAgentRuntime {
    pub(crate) store: Arc<dyn ModelAgentStore>,
    pub(crate) executor: Option<Arc<ModelAgentExecutor>>,
    pub(crate) planning_version: String,
    pub(crate) execution_version: String,
}
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/conversations/{id}/model-agents",
            get(list).post(create),
        )
        .route("/api/conversations/{id}/model-agents/{request}", get(read))
        .route(
            "/api/conversations/{id}/model-agents/{request}/{action}",
            post(action),
        )
}
#[allow(clippy::needless_pass_by_value)] // 直接用于 Result::map_err。
fn error(e: StorageError) -> ApiError {
    match e {
        StorageError::NotFound => ApiError(StatusCode::NOT_FOUND, "model_agent_not_found"),
        StorageError::InvalidData(_) => ApiError(StatusCode::BAD_REQUEST, "invalid_model_agent"),
        StorageError::Conflict(_) => ApiError(StatusCode::CONFLICT, "model_agent_conflict"),
        StorageError::Unavailable(_) => {
            ApiError(StatusCode::SERVICE_UNAVAILABLE, "model_agent_unavailable")
        }
    }
}
fn key(id: &str) -> Result<String, ApiError> {
    Uuid::parse_str(id)
        .map(|v| v.to_string())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_model_agent_id"))
}
fn runtime(s: &AppState) -> Result<&Arc<ModelAgentRuntime>, ApiError> {
    s.model_agents.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "model_agent_unavailable",
    ))
}
fn enabled(r: &ModelAgentRuntime) -> Result<&Arc<ModelAgentExecutor>, ApiError> {
    r.executor
        .as_ref()
        .filter(|e| !e.is_halted())
        .ok_or(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "model_agent_disabled",
        ))
}
// 金额为十进制微单位字符串，避免浏览器将 i64 转成不精确的 Number。
fn view(value: &impl Serialize) -> Result<Value, ApiError> {
    let mut v = serde_json::to_value(value)
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "model_agent_unavailable"))?;
    let object = v.as_object_mut().ok_or(ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "model_agent_unavailable",
    ))?;
    let amount = object
        .remove("amount")
        .and_then(|v| v.as_i64())
        .ok_or(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "model_agent_unavailable",
        ))?;
    object.insert("amount_micro".into(), json!(amount.to_string()));
    Ok(v)
}
async fn item(
    r: &ModelAgentRuntime,
    owner: &personal_ai_domain::UserId,
    conversation: &str,
    request: &str,
) -> Result<Value, ApiError> {
    let planning = r
        .store
        .get_model_planning_request(owner, conversation, request)
        .await
        .map_err(error)?;
    let execution = match r
        .store
        .get_model_execution_request(owner, conversation, request)
        .await
    {
        Ok(result) => view(&result)?,
        Err(StorageError::NotFound) => Value::Null,
        Err(e) => return Err(error(e)),
    };
    Ok(json!({"planning":view(&planning)?,"execution":execution}))
}
async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&s, &headers).await?;
    let id = key(&id)?;
    s.conversations
        .get_conversation(&user.id, &id)
        .await
        .map_err(error)?;
    let mut items = Vec::new();
    if let Some(r) = &s.model_agents {
        for planning in r
            .store
            .list_model_planning_requests(&user.id, &id)
            .await
            .map_err(error)?
        {
            items.push(item(r, &user.id, &id, &planning.request_id).await?);
        }
    }
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(
            json!({"enabled":s.model_agents.as_ref().is_some_and(|r| enabled(r).is_ok()),"items":items}),
        ),
    ))
}
async fn read(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((id, request)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&s, &headers).await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(item(runtime(&s)?, &user.id, &key(&id)?, &key(&request)?).await?),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    request_id: Uuid,
    expected_revision: i64,
}
async fn create(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    payload: Result<Json<Create>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&s, &headers).await?;
    auth::mutation_guard(&headers)?;
    let Json(input) = payload.map_err(|e| ApiError(e.status(), "invalid_model_agent"))?;
    let r = runtime(&s)?;
    enabled(r)?;
    let request = r
        .store
        .create_model_planning_request(
            &user.id,
            &key(&id)?,
            &NewModelPlanningRequest {
                request_id: input.request_id.to_string(),
                expected_revision: input.expected_revision,
                configuration_version: r.planning_version.clone(),
            },
        )
        .await
        .map_err(error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(view(&request)?)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approve {
    digest: String,
    accepted_currency: String,
    accepted_amount_micro: String,
    accepted_calls: AgentCallCounts,
    acknowledge_cost: bool,
}
fn approval(value: Value) -> Result<AgentQuoteApproval, ApiError> {
    let bad = || ApiError(StatusCode::BAD_REQUEST, "invalid_model_agent_approval");
    let input: Approve = serde_json::from_value(value).map_err(|_| bad())?;
    let amount = input
        .accepted_amount_micro
        .parse::<i64>()
        .map_err(|_| bad())?;
    if amount <= 0 || amount.to_string() != input.accepted_amount_micro {
        return Err(bad());
    }
    Ok(AgentQuoteApproval {
        digest: input.digest,
        accepted_currency: input.accepted_currency,
        accepted_amount: amount,
        accepted_calls: input.accepted_calls,
        acknowledge_cost: input.acknowledge_cost,
    })
}
async fn action(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((id, request, action)): Path<(String, String, String)>,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&s, &headers).await?;
    auth::mutation_guard(&headers)?;
    let Json(body) = payload.map_err(|e| ApiError(e.status(), "invalid_model_agent"))?;
    let (id, request) = (key(&id)?, key(&request)?);
    let r = runtime(&s)?;
    let value = match action.as_str() {
        "approve-planning" => view(
            &enabled(r)?
                .start_planning(&user.id, &id, &request, &approval(body)?)
                .await
                .map_err(error)?,
        )?,
        "approve-execution" => view(
            &enabled(r)?
                .start_execution(&user.id, &id, &request, &approval(body)?)
                .await
                .map_err(error)?,
        )?,
        "preview-execution" | "cancel-planning" | "cancel-execution" => {
            if body != json!({}) {
                return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_model_agent"));
            }
            match action.as_str() {
                "preview-execution" => {
                    enabled(r)?;
                    view(
                        &r.store
                            .create_model_execution_request(
                                &user.id,
                                &id,
                                &request,
                                &r.execution_version,
                            )
                            .await
                            .map_err(error)?,
                    )?
                }
                "cancel-planning" => view(
                    &r.store
                        .cancel_model_planning_request(&user.id, &id, &request)
                        .await
                        .map_err(error)?,
                )?,
                _ => view(
                    &r.store
                        .cancel_model_execution_request(&user.id, &id, &request)
                        .await
                        .map_err(error)?,
                )?,
            }
        }
        _ => return Err(ApiError(StatusCode::NOT_FOUND, "not_found")),
    };
    Ok((
        StatusCode::ACCEPTED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(value),
    ))
}
