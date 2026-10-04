//! Preparation only: these routes never dispatch an answer or claim a grant.
use crate::{ApiError, AppState, auth};
use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    StorageError, StorageResult,
    answer_authorizations::{AnswerApproval, AnswerAuthorizationStore, AnswerPreparation},
};
use serde::Serialize;
use std::{future::Future, time::Duration};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/knowledge/answer-authorizations",
            get(list).post(prepare),
        )
        .route("/api/knowledge/answer-authorizations/{id}", get(detail))
        .route(
            "/api/knowledge/answer-authorizations/{id}/approve",
            post(approve),
        )
        .route(
            "/api/knowledge/answer-authorizations/{id}/cancel",
            post(cancel),
        )
}
fn unavailable() -> ApiError {
    ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "answer_authorization_unavailable",
    )
}
fn store(state: &AppState) -> Result<&dyn AnswerAuthorizationStore, ApiError> {
    state
        .answer_authorizations
        .as_deref()
        .ok_or_else(unavailable)
}
fn map_error(error: &StorageError) -> ApiError {
    match error {
        StorageError::NotFound => ApiError(StatusCode::NOT_FOUND, "answer_authorization_not_found"),
        StorageError::InvalidData(_) => {
            ApiError(StatusCode::BAD_REQUEST, "invalid_answer_authorization")
        }
        StorageError::Conflict(_) => {
            ApiError(StatusCode::CONFLICT, "answer_authorization_conflict")
        }
        StorageError::Unavailable(_) => unavailable(),
    }
}
fn body<T>(input: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    input
        .map(|Json(value)| value)
        .map_err(|e| ApiError(e.status(), "invalid_answer_authorization"))
}
async fn finish<T: Serialize>(
    state: &AppState,
    headers: &HeaderMap,
    owner: &UserId,
    operation: impl Future<Output = StorageResult<T>>,
) -> Result<Response, ApiError> {
    let value = tokio::time::timeout(Duration::from_secs(15), operation)
        .await
        .map_err(|_| unavailable())?
        .map_err(|e| map_error(&e))?;
    if auth::current_user(state, headers).await?.id != *owner {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response())
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let owner = auth::current_user(&state, &headers).await?.id;
    let store = store(&state)?;
    finish(&state, &headers, &owner, async {
        let items = store.list_answer_authorizations(&owner).await?;
        Ok(serde_json::json!({"items":items,"execution_available":false}))
    })
    .await
}
async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let owner = auth::current_user(&state, &headers).await?.id;
    finish(
        &state,
        &headers,
        &owner,
        store(&state)?.get_answer_authorization(&owner, &id),
    )
    .await
}
async fn prepare(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<AnswerPreparation>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let owner = auth::current_user(&state, &headers).await?.id;
    auth::mutation_guard(&headers)?;
    let input = body(payload)?;
    finish(
        &state,
        &headers,
        &owner,
        store(&state)?.prepare_answer(&owner, &input),
    )
    .await
}
async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    payload: Result<Json<AnswerApproval>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let owner = auth::current_user(&state, &headers).await?.id;
    auth::mutation_guard(&headers)?;
    let input = body(payload)?;
    finish(
        &state,
        &headers,
        &owner,
        store(&state)?.approve_answer(&owner, &id, &input),
    )
    .await
}
async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let owner = auth::current_user(&state, &headers).await?.id;
    auth::mutation_guard(&headers)?;
    finish(
        &state,
        &headers,
        &owner,
        store(&state)?.cancel_answer(&owner, &id),
    )
    .await
}
