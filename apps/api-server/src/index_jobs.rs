use crate::{ApiError, AppState, Indexing, auth};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use personal_ai_domain::UserId;
use personal_ai_knowledge::index::IndexError;
use personal_ai_llm::LlmError;
use personal_ai_storage::{
    StorageError,
    documents::DocumentStore,
    index_jobs::{BatchOutcome, IndexJob, IndexJobStore},
};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/api/documents/{id}/index-job", get(status).post(enqueue))
}
async fn request(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    enqueue: bool,
) -> Result<IndexJob, ApiError> {
    let user = auth::current_user(state, headers).await?;
    if enqueue {
        auth::mutation_guard(headers)?;
    }
    let id = Uuid::parse_str(id)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_document_id"))?
        .to_string();
    let indexing = state.indexing.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "indexing_disabled",
    ))?;
    let jobs = indexing.jobs.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "indexing_disabled",
    ))?;
    let result = if enqueue {
        jobs.enqueue(&user.id, &id, &indexing.target).await
    } else {
        jobs.index_status(&user.id, &id, &indexing.target).await
    };
    result.map_err(|error| match error {
        StorageError::NotFound => ApiError(StatusCode::NOT_FOUND, "index_job_not_found"),
        other => other.into(),
    })
}
async fn enqueue(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let job = request(&state, &headers, &id, true).await?;
    Ok((
        StatusCode::ACCEPTED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(job),
    ))
}
async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let job = request(&state, &headers, &id, false).await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(job)))
}

impl Indexing {
    /// 持久化队列执行器；终止时未确认批次由租约过期恢复，可能重复计费。
    pub async fn run_jobs(&self, documents: Arc<dyn DocumentStore>) {
        let Some(jobs) = &self.jobs else {
            return;
        };
        loop {
            if self
                .run_one_job(jobs.as_ref(), documents.as_ref())
                .await
                .is_err()
            {
                tracing::warn!("index job storage operation failed");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    async fn run_one_job(
        &self,
        jobs: &dyn IndexJobStore,
        documents: &dyn DocumentStore,
    ) -> Result<(), StorageError> {
        let Ok(_permit) = self.slots.try_acquire() else {
            return Ok(());
        };
        let Some(job) = jobs.claim(&self.target).await? else {
            return Ok(());
        };
        let outcome = tokio::time::timeout(Duration::from_secs(35), async {
            let owner = UserId::new(job.owner.clone());
            let document = match documents.get_document(&owner, &job.document_id).await {
                Ok(document) => document,
                Err(StorageError::NotFound | StorageError::InvalidData(_)) => {
                    return BatchOutcome::Failed("document_unavailable");
                }
                Err(_) => return BatchOutcome::Retry("document_unavailable"),
            };
            let Ok(offset) = usize::try_from(job.indexed_chunks) else {
                return BatchOutcome::Failed("invalid_progress");
            };
            match self.indexer.index_batch(&owner, &document, offset).await {
                Ok(batch) => match i32::try_from(offset + batch.indexed_chunks) {
                    Ok(end) => BatchOutcome::Success(end),
                    Err(_) => BatchOutcome::Failed("invalid_progress"),
                },
                Err(IndexError::InvalidOffset) => BatchOutcome::Failed("invalid_progress"),
                Err(IndexError::Model(
                    LlmError::InvalidRequest(_) | LlmError::InvalidResponse(_),
                )) => BatchOutcome::Failed("embedding_invalid"),
                Err(IndexError::Model(_)) => BatchOutcome::Retry("embedding_unavailable"),
                Err(IndexError::Storage(_)) => BatchOutcome::Retry("vector_store_unavailable"),
            }
        })
        .await
        .unwrap_or(BatchOutcome::Retry("indexing_timeout"));
        jobs.finish_batch(&self.target, &job, outcome).await
    }
}
