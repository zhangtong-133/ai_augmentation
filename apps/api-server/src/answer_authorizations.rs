//! Exact preview, explicit consent and owner-scoped, one-time local execution.
mod execution;
use crate::{ApiError, AppState, auth};
use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    middleware,
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
        .route(
            "/api/knowledge/answer-authorizations/{id}/execute",
            post(execution::execute),
        )
        .layer(middleware::map_response(no_store))
}
async fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
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
        execution::list_view(&state, &owner, items).await
    })
    .await
}
async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let owner = auth::current_user(&state, &headers).await?.id;
    let store = store(&state)?;
    finish(&state, &headers, &owner, async {
        let item = store.get_answer_authorization(&owner, &id).await?;
        execution::view(&state, &owner, item).await
    })
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
    let store = store(&state)?;
    finish(&state, &headers, &owner, async {
        let item = store.prepare_answer(&owner, &input).await?;
        execution::view(&state, &owner, item).await
    })
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
    let store = store(&state)?;
    finish(&state, &headers, &owner, async {
        let item = store.approve_answer(&owner, &id, &input).await?;
        execution::view(&state, &owner, item).await
    })
    .await
}
async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let owner = auth::current_user(&state, &headers).await?.id;
    auth::mutation_guard(&headers)?;
    let store = store(&state)?;
    finish(&state, &headers, &owner, async {
        let item = match store.cancel_answer(&owner, &id).await {
            Ok(item) => item,
            Err(StorageError::Conflict(_)) => {
                // A concurrent execute may have consumed the authorization first.
                // Apply this user cancellation to its durable execution instead.
                let existing = store.get_answer_authorization(&owner, &id).await?;
                if existing.status != "consumed" {
                    return Err(StorageError::Conflict(
                        "answer authorization changed".into(),
                    ));
                }
                let executions = state
                    .answer_executions
                    .as_deref()
                    .ok_or_else(|| StorageError::Conflict("answer execution unavailable".into()))?;
                executions.cancel_answer_execution(&owner, &id).await?;
                existing
            }
            Err(error) => return Err(error),
        };
        execution::view(&state, &owner, item).await
    })
    .await
}
