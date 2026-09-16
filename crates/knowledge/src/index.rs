use personal_ai_domain::UserId;
use personal_ai_llm::{EmbeddingProvider, LlmError};
use personal_ai_storage::{EmbeddingRecord, StorageError, VectorStore, documents::StoredDocument};
use std::sync::Arc;

pub enum IndexError {
    Model(LlmError),
    Storage(StorageError),
    InvalidOffset,
}
pub struct IndexBatch {
    pub indexed_chunks: usize,
    pub next_offset: Option<usize>,
    pub total_chunks: usize,
}
pub struct DocumentIndexer {
    provider: Arc<dyn EmbeddingProvider>,
    vectors: Arc<dyn VectorStore>,
    model: String,
    dimensions: usize,
}
impl DocumentIndexer {
    #[must_use]
    pub fn new(
        provider: Arc<dyn EmbeddingProvider>,
        vectors: Arc<dyn VectorStore>,
        model: String,
        dimensions: usize,
    ) -> Self {
        Self {
            provider,
            vectors,
            model,
            dimensions,
        }
    }
    /// 为已认证所有者的文档索引最多 16 块，重复调用同一批次覆盖相同点。
    ///
    /// # Errors
    /// 偏移越界、模型返回无效数据或任一依赖失败时返回错误，不返回虚假成功。
    pub async fn index_batch(
        &self,
        owner: &UserId,
        document: &StoredDocument,
        offset: usize,
    ) -> Result<IndexBatch, IndexError> {
        let total = document.chunks.len();
        if offset >= total {
            return Err(IndexError::InvalidOffset);
        }
        let end = (offset + 16).min(total);
        let chunks = &document.chunks[offset..end];
        let embeddings = self
            .provider
            .embedding(chunks)
            .await
            .map_err(IndexError::Model)?;
        if embeddings.len() != chunks.len()
            || embeddings.iter().any(|e| {
                e.model != self.model
                    || e.values.len() != self.dimensions
                    || e.values.iter().any(|v| !v.is_finite())
                    || !e.values.iter().any(|v| *v != 0.0)
            })
        {
            return Err(IndexError::Model(LlmError::InvalidResponse(
                "invalid embedding batch".into(),
            )));
        }
        let created_at_unix_ms =
            u64::try_from(document.summary.created_at_unix_ms).map_err(|_| {
                IndexError::Storage(StorageError::InvalidData(
                    "invalid document timestamp".into(),
                ))
            })?;
        let records: Vec<_> = embeddings
            .into_iter()
            .zip(chunks)
            .enumerate()
            .map(|(index, (embedding, text))| {
                let ordinal = offset + index;
                EmbeddingRecord {
                    id: format!("{}:{ordinal}", document.summary.id),
                    document_id: document.summary.id.clone(),
                    ordinal,
                    text: text.clone(),
                    model: self.model.clone(),
                    vector: embedding.values,
                    source: document.summary.source.clone(),
                    content_type: document.summary.source_type.clone(),
                    created_at_unix_ms,
                    tags: document.summary.tags.clone(),
                }
            })
            .collect();
        self.vectors
            .insert_embeddings(owner, &records)
            .await
            .map_err(IndexError::Storage)?;
        Ok(IndexBatch {
            indexed_chunks: records.len(),
            next_offset: (end < total).then_some(end),
            total_chunks: total,
        })
    }
}
