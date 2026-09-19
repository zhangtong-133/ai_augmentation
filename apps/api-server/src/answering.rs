use crate::{ApiError, AppState, auth, retrieval};
use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::post,
};
use personal_ai_llm::{AnswerProvider, LlmError};
use serde::Deserialize;
use std::{sync::Arc, time::Duration};

/// 知识问答独立显式启用，复用服务端 `OpenAI` 凭据。
///
/// # Errors
/// 未启用检索、聊天模型缺失或配置无效时启动失败。
pub fn answering_from_env(
    indexing_enabled: bool,
) -> Result<Option<Arc<dyn AnswerProvider>>, Box<dyn std::error::Error>> {
    match std::env::var("KNOWLEDGE_ANSWER_ENABLED").as_deref() {
        Err(std::env::VarError::NotPresent) | Ok("false") => return Ok(None),
        Ok("true") => {}
        _ => return Err("KNOWLEDGE_ANSWER_ENABLED must be true or false".into()),
    }
    if !indexing_enabled {
        return Err("knowledge answers require indexing".into());
    }
    let required = |key: &str| {
        std::env::var(key)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| format!("{key} is required"))
    };
    Ok(Some(Arc::new(personal_ai_llm_openai::OpenAiAnswers::new(
        &std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into()),
        &required("OPENAI_API_KEY")?,
        &required("OPENAI_CHAT_MODEL")?,
    )?)))
}
pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/api/knowledge/answer", post(answer))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerRequest {
    query: String,
}
async fn answer(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<AnswerRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let Json(input) = payload.map_err(|e| ApiError(e.status(), "invalid_json"))?;
    let provider = state.answering.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "answering_disabled",
    ))?;
    let indexing = state.indexing.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "indexing_disabled",
    ))?;
    let _permit = indexing
        .slots
        .try_acquire()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "indexing_busy"))?;
    let response = tokio::time::timeout(Duration::from_secs(55), async {
        let hits = tokio::time::timeout(
            Duration::from_secs(35),
            indexing
                .indexer
                .search(&user.id, state.documents.as_ref(), &input.query, 5),
        )
        .await
        .map_err(|_| ApiError(StatusCode::GATEWAY_TIMEOUT, "retrieval_timeout"))?
        .map_err(|e| retrieval::error(&e))?;
        personal_ai_knowledge::answer::answer(provider.as_ref(), &input.query, &hits)
            .await
            .map_err(|error| match error {
                LlmError::RateLimited => {
                    ApiError(StatusCode::TOO_MANY_REQUESTS, "answer_rate_limited")
                }
                LlmError::InvalidResponse(_) => ApiError(StatusCode::BAD_GATEWAY, "invalid_answer"),
                _ => ApiError(StatusCode::BAD_GATEWAY, "answer_unavailable"),
            })
    })
    .await
    .map_err(|_| ApiError(StatusCode::GATEWAY_TIMEOUT, "answer_timeout"))??;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(response)))
}
