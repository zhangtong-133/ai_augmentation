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
use personal_ai_storage::StorageError;
use serde::Deserialize;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use uuid::Uuid;

pub struct Indexing {
    indexer: DocumentIndexer,
    slots: Semaphore,
}
impl Indexing {
    #[must_use]
    pub fn new(indexer: DocumentIndexer) -> Self {
        Self {
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
    let provider = personal_ai_llm_openai::OpenAiEmbeddings::new(
        &std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into()),
        &required("OPENAI_API_KEY")?,
        &model,
        dimensions,
    )?;
    let vectors = personal_ai_storage_qdrant::QdrantStore::new(
        &required("QDRANT_URL")?,
        &required("QDRANT_COLLECTION")?,
        std::env::var("QDRANT_API_KEY")
            .ok()
            .filter(|v| !v.is_empty())
            .as_deref(),
        &model,
        dimensions,
    )?;
    vectors.ensure_collection().await?;
    Ok(Some(Arc::new(Indexing::new(DocumentIndexer::new(
        Arc::new(provider),
        Arc::new(vectors),
        model,
        dimensions,
    )))))
}
pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/api/documents/{id}/index", post(index))
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
