use crate::{ApiError, AppState, auth, tool_execution};
use axum::{
    Extension, Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use personal_ai_domain::UserId;
use personal_ai_storage::{StorageError, mcp_credentials::NewMcpCredential};
use personal_ai_tools::ToolExecutor;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

pub(super) fn routes(executor: Arc<ToolExecutor>) -> Router<AppState> {
    Router::new()
        .route("/api/mcp/credentials", get(list).post(create))
        .route("/api/mcp/credentials/{id}/revoke", post(revoke))
        .route("/api/mcp/tools", get(tools))
        .route("/api/mcp/tools/knowledge_search", post(search))
        .layer(Extension(executor))
}
fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    host_name: String,
    expires_in_days: i32,
    acknowledge_embedding_cost: bool,
}
async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<Create>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let Json(input) = payload.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_json"))?;
    if !input.acknowledge_embedding_cost {
        return Err(ApiError(StatusCode::BAD_REQUEST, "cost_consent_required"));
    }
    let token = format!(
        "pai_mcp_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    );
    let input = NewMcpCredential {
        digest: digest(&token),
        session_digest: auth::session_digest(&headers)?,
        host_name: input.host_name,
        days: input.expires_in_days,
    };
    if !input.valid() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_mcp_credential"));
    }
    let credential = state
        .store
        .create_mcp_credential(&user.id, &input)
        .await
        .map_err(storage_error)?;
    Ok((
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"credential":credential,"token":token})),
    ))
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let items = state
        .store
        .list_mcp_credentials(&user.id)
        .await
        .map_err(storage_error)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"items":items})),
    ))
}
async fn revoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    state
        .store
        .revoke_mcp_credential(&user.id, &id)
        .await
        .map_err(storage_error)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"status":"revoked"})),
    ))
}
async fn owner(state: &AppState, headers: &HeaderMap) -> Result<UserId, ApiError> {
    if headers.contains_key(header::COOKIE)
        || headers.get_all(header::AUTHORIZATION).iter().count() != 1
    {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "invalid_mcp_credential"));
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| {
            v.strip_prefix("pai_mcp_")
                .is_some_and(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        })
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "invalid_mcp_credential"))?;
    state
        .store
        .mcp_credential_owner(&digest(token))
        .await
        .map_err(|error| {
            if matches!(error, StorageError::NotFound) {
                ApiError(StatusCode::UNAUTHORIZED, "invalid_mcp_credential")
            } else {
                storage_error(error)
            }
        })
}
async fn tools(
    State(state): State<AppState>,
    Extension(executor): Extension<Arc<ToolExecutor>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    owner(&state, &headers).await?;
    Ok(tool_execution::manifest(&executor))
}
async fn search(
    State(state): State<AppState>,
    Extension(executor): Extension<Arc<ToolExecutor>>,
    headers: HeaderMap,
    payload: Result<Json<Value>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let owner = owner(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    tool_execution::execute_for(
        state,
        executor,
        owner,
        "knowledge_search".into(),
        headers,
        payload,
    )
    .await
}
#[allow(clippy::needless_pass_by_value)] // Result::map_err consumes the storage error.
fn storage_error(error: StorageError) -> ApiError {
    match error {
        StorageError::NotFound => ApiError(StatusCode::NOT_FOUND, "mcp_credential_not_found"),
        StorageError::InvalidData(_) => ApiError(StatusCode::BAD_REQUEST, "invalid_mcp_credential"),
        StorageError::Conflict(_) => {
            ApiError(StatusCode::TOO_MANY_REQUESTS, "mcp_credential_limit")
        }
        StorageError::Unavailable(_) => ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "mcp_credentials_unavailable",
        ),
    }
}
