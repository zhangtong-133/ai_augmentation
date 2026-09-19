use crate::{ApiError, AppState, auth};
use axum::{
    Json, Router,
    extract::{Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::post,
};
use personal_ai_knowledge::index::{DocumentIndexer, IndexError};
use personal_ai_llm::LlmError;
use personal_ai_storage::{
    StorageError, StorageResult,
    documents::DocumentStore,
    index_jobs::{IndexFailure, IndexJobStore},
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use uuid::Uuid;

pub struct Indexing {
    indexer: DocumentIndexer,
    slots: Semaphore,
    profile: String,
}
impl Indexing {
    #[must_use]
    pub fn new(indexer: DocumentIndexer) -> Self {
        let profile = format!(
            "{:x}",
            Sha256::digest(format!("{}:{}", indexer.model(), indexer.dimensions()))
        );
        Self {
            profile,
            indexer,
            slots: Semaphore::new(2),
        }
    }
}
/// 显式启用索引时才读取模型凭据、连接向量库；缺失配置启动失败。
///
/// # Errors
/// 配置无效、向量库不可用或集合维度不匹配时返回错误。
pub async fn indexing_from_env() -> Result<Option<Arc<Indexing>>, Box<dyn std::error::Error>> {
    match std::env::var("KNOWLEDGE_INDEX_ENABLED").as_deref() {
        Err(std::env::VarError::NotPresent) | Ok("false") => return Ok(None),
        Ok("true") => {}
        _ => return Err("KNOWLEDGE_INDEX_ENABLED must be true or false".into()),
    }
    let required = |key: &str| {
        std::env::var(key)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| format!("{key} is required"))
    };
    let model = required("OPENAI_EMBEDDING_MODEL")?;
    let dimensions: usize = required("EMBEDDING_DIMENSIONS")?
        .parse()
        .map_err(|_| "invalid EMBEDDING_DIMENSIONS")?;
    let base_url =
        std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());
    let qdrant_url = required("QDRANT_URL")?;
    let collection = required("QDRANT_COLLECTION")?;
    let profile = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&json!([
            base_url.trim_end_matches('/'),
            model,
            dimensions,
            qdrant_url.trim_end_matches('/'),
            collection
        ]))?)
    );
    let provider = personal_ai_llm_openai::OpenAiEmbeddings::new(
        &base_url,
        &required("OPENAI_API_KEY")?,
        &model,
        dimensions,
    )?;
    let vectors = personal_ai_storage_qdrant::QdrantStore::new(
        &qdrant_url,
        &collection,
        std::env::var("QDRANT_API_KEY")
            .ok()
            .filter(|v| !v.is_empty())
            .as_deref(),
        &model,
        dimensions,
    )?;
    vectors.ensure_collection().await?;
    let mut indexing = Indexing::new(DocumentIndexer::new(
        Arc::new(provider),
        Arc::new(vectors),
        model,
        dimensions,
    ));
    indexing.profile = profile;
    Ok(Some(Arc::new(indexing)))
}
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/documents/{id}/index", post(index))
        .route("/api/documents/{id}/index-jobs", post(enqueue).get(status))
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Batch {
    #[serde(default)]
    offset: usize,
}
async fn index(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    query: Result<Query<Batch>, QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let id = Uuid::parse_str(&id)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_document_id"))?
        .to_string();
    let Query(batch) =
        query.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_index_offset"))?;
    let indexing = state.indexing.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "indexing_disabled",
    ))?;
    let _permit = indexing
        .slots
        .try_acquire()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "indexing_busy"))?;
    let result = tokio::time::timeout(Duration::from_secs(35), async {
        // 所有权校验必须先于任何模型调用或向量写入。
        let document = state
            .documents
            .get_document(&user.id, &id)
            .await
            .map_err(|error| match error {
                StorageError::NotFound => ApiError(StatusCode::NOT_FOUND, "document_not_found"),
                other => other.into(),
            })?;
        indexing
            .indexer
            .index_batch(&user.id, &document, batch.offset)
            .await
            .map_err(|error| match error {
                IndexError::InvalidOffset => {
                    ApiError(StatusCode::BAD_REQUEST, "invalid_index_offset")
                }
                IndexError::Model(LlmError::RateLimited) => {
                    ApiError(StatusCode::TOO_MANY_REQUESTS, "embedding_rate_limited")
                }
                IndexError::Model(_) => ApiError(StatusCode::BAD_GATEWAY, "embedding_unavailable"),
                IndexError::Storage(_) => {
                    ApiError(StatusCode::SERVICE_UNAVAILABLE, "vector_store_unavailable")
                }
            })
    })
    .await
    .map_err(|_| ApiError(StatusCode::GATEWAY_TIMEOUT, "indexing_timeout"))??;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(
            json!({"document_id": id, "indexed_chunks": result.indexed_chunks, "next_offset": result.next_offset, "total_chunks": result.total_chunks}),
        ),
    ))
}

fn job_error(error: StorageError) -> ApiError {
    match error {
        StorageError::NotFound => ApiError(StatusCode::NOT_FOUND, "document_not_found"),
        other => other.into(),
    }
}
async fn enqueue(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let id = Uuid::parse_str(&id)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_document_id"))?
        .to_string();
    let indexing = state.indexing.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "indexing_disabled",
    ))?;
    let jobs = state.index_jobs.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "indexing_disabled",
    ))?;
    let job = jobs
        .enqueue_index(&user.id, &id, &indexing.profile)
        .await
        .map_err(job_error)?;
    let status = if job.status == "succeeded" {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };
    Ok((
        status,
        [
            (header::CACHE_CONTROL, "no-store".to_owned()),
            (header::LOCATION, format!("/api/documents/{id}/index-jobs")),
        ],
        Json(job),
    ))
}
async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let id = Uuid::parse_str(&id)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_document_id"))?
        .to_string();
    let indexing = state.indexing.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "indexing_disabled",
    ))?;
    let jobs = state.index_jobs.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "indexing_disabled",
    ))?;
    let job = jobs
        .index_status(&user.id, &id, &indexing.profile)
        .await
        .map_err(job_error)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"job": job})),
    ))
}

/// 使用数据库租约逐批处理持久化任务；进程取消后由租约过期机制恢复。
pub async fn run_index_jobs(
    indexing: Arc<Indexing>,
    jobs: Arc<dyn IndexJobStore>,
    documents: Arc<dyn DocumentStore>,
) {
    loop {
        match process_next_job(&indexing, jobs.as_ref(), documents.as_ref()).await {
            Ok(true) => continue,
            Ok(false) => {}
            Err(_) => tracing::warn!("index job storage operation failed; will retry polling"),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
async fn process_next_job(
    indexing: &Indexing,
    jobs: &dyn IndexJobStore,
    documents: &dyn DocumentStore,
) -> StorageResult<bool> {
    let Ok(_permit) = indexing.slots.try_acquire() else {
        return Ok(false);
    };
    let Some(lease) = jobs.claim_index(&indexing.profile).await? else {
        return Ok(false);
    };
    let result = tokio::time::timeout(Duration::from_secs(35), async {
        let document = documents
            .get_document(&lease.owner, &lease.job.document_id)
            .await
            .map_err(|error| match error {
                StorageError::Unavailable(_) => IndexFailure::DocumentUnavailable,
                _ => IndexFailure::InvalidDocument,
            })?;
        if i32::try_from(document.chunks.len()).ok() != Some(lease.job.total_chunks) {
            return Err(IndexFailure::InvalidDocument);
        }
        let offset =
            usize::try_from(lease.job.indexed_chunks).map_err(|_| IndexFailure::InvalidDocument)?;
        let batch = indexing
            .indexer
            .index_batch(&lease.owner, &document, offset)
            .await
            .map_err(|error| match error {
                IndexError::Model(LlmError::InvalidRequest(_) | LlmError::InvalidResponse(_)) => {
                    IndexFailure::InvalidEmbedding
                }
                IndexError::Model(LlmError::RateLimited) => IndexFailure::ModelRateLimited,
                IndexError::Model(LlmError::ProviderUnavailable(_)) => {
                    IndexFailure::ModelUnavailable
                }
                IndexError::InvalidOffset | IndexError::Storage(StorageError::InvalidData(_)) => {
                    IndexFailure::InvalidDocument
                }
                IndexError::Storage(_) => IndexFailure::VectorUnavailable,
            })?;
        i32::try_from(offset + batch.indexed_chunks).map_err(|_| IndexFailure::InvalidDocument)
    })
    .await
    .unwrap_or(Err(IndexFailure::Timeout));
    match result {
        Ok(indexed) => jobs.complete_index_batch(&lease, indexed).await?,
        Err(failure) => jobs.fail_index_batch(&lease, failure).await?,
    }
    Ok(true)
}
