use crate::{ApiError, AppState, auth};
use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    request_id: Uuid,
    title: String,
}
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/conversations", get(list).post(create))
        .route("/api/conversations/{id}", get(read).delete(remove))
}
fn id(value: &str) -> Result<String, ApiError> {
    Uuid::parse_str(value)
        .map(|value| value.to_string())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_conversation"))
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let records = state.conversations.list_conversations(&user.id).await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(records)))
}
async fn read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let record = state
        .conversations
        .get_conversation(&user.id, &id(&key)?)
        .await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(record)))
}
async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<Create>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let Json(input) = payload.map_err(|error| ApiError(error.status(), "invalid_conversation"))?;
    if input.title.trim().is_empty()
        || input.title.chars().count() > 80
        || input.title.contains('\0')
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_conversation"));
    }
    let record = state
        .conversations
        .create_conversation(&user.id, &input.request_id.to_string(), &input.title)
        .await?;
    // 首次与幂等重放均返回 200，响应内容保持一致。
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(record)))
}
async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    state
        .conversations
        .delete_conversation(&user.id, &id(&key)?)
        .await?;
    Ok((
        StatusCode::NO_CONTENT,
        [(header::CACHE_CONTROL, "no-store")],
    ))
}
