use crate::{ApiError, AppState, auth};
use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::post,
};
use personal_ai_knowledge::index::IndexError;
use personal_ai_llm::LlmError;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/api/knowledge/search", post(search))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchRequest {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    5
}
pub(super) fn error(error: &IndexError) -> ApiError {
    match error {
        IndexError::Model(LlmError::RateLimited) => {
            ApiError(StatusCode::TOO_MANY_REQUESTS, "embedding_rate_limited")
        }
        IndexError::Model(LlmError::InvalidRequest(_)) | IndexError::InvalidOffset => {
            ApiError(StatusCode::BAD_REQUEST, "invalid_query")
        }
        IndexError::Model(_) => ApiError(StatusCode::BAD_GATEWAY, "embedding_unavailable"),
        IndexError::Storage(_) => {
            ApiError(StatusCode::SERVICE_UNAVAILABLE, "retrieval_unavailable")
        }
    }
}
async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<SearchRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let Json(input) = payload.map_err(|e| ApiError(e.status(), "invalid_json"))?;
    let indexing = state.indexing.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "indexing_disabled",
    ))?;
    let _permit = indexing
        .slots
        .try_acquire()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "indexing_busy"))?;
    let hits = tokio::time::timeout(
        Duration::from_secs(35),
        indexing.indexer.search(
            &user.id,
            state.documents.as_ref(),
            &input.query,
            input.limit,
        ),
    )
    .await
    .map_err(|_| ApiError(StatusCode::GATEWAY_TIMEOUT, "retrieval_timeout"))?
    .map_err(|e| error(&e))?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"hits": hits})),
    ))
}
