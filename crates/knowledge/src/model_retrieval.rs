//! 已付费向量的只读召回适配；复用知识库归属、模型与文本核验，不再次向量化。
use personal_ai_agent_core::{BoxFuture, model_executor::ModelRetriever};
use personal_ai_domain::UserId;
use personal_ai_llm::Embedding;
use personal_ai_storage::{
    StorageError, StorageResult, VectorStore, documents::DocumentStore,
    model_execution::RetrievedChunk,
};
use std::sync::Arc;

pub struct KnowledgeModelRetriever {
    documents: Arc<dyn DocumentStore>,
    vectors: Arc<dyn VectorStore>,
    model: String,
    dimensions: usize,
}
impl KnowledgeModelRetriever {
    /// 模型与维度必须和既有知识库索引配置一致。
    /// # Errors
    /// 拒绝空模型或无效维度。
    pub fn new(
        documents: Arc<dyn DocumentStore>,
        vectors: Arc<dyn VectorStore>,
        model: String,
        dimensions: usize,
    ) -> StorageResult<Self> {
        if model.trim().is_empty() || !(1..=4096).contains(&dimensions) {
            return Err(StorageError::InvalidData(
                "invalid model retrieval configuration".into(),
            ));
        }
        Ok(Self {
            documents,
            vectors,
            model,
            dimensions,
        })
    }
}
impl ModelRetriever for KnowledgeModelRetriever {
    fn retrieve<'a>(
        &'a self,
        owner: &'a UserId,
        embedding: &'a Embedding,
        limit: usize,
    ) -> BoxFuture<'a, StorageResult<Vec<RetrievedChunk>>> {
        Box::pin(async move {
            if !(1..=5).contains(&limit) {
                return Err(StorageError::InvalidData(
                    "invalid model retrieval limit".into(),
                ));
            }
            let hits = crate::retrieval::retrieve(
                owner,
                &*self.documents,
                &*self.vectors,
                embedding,
                &self.model,
                self.dimensions,
                limit,
            )
            .await
            .map_err(|_| StorageError::Unavailable("model retrieval unavailable".into()))?;
            Ok(hits
                .into_iter()
                .map(|hit| RetrievedChunk {
                    document_id: hit.document_id,
                    ordinal: hit.ordinal,
                    text: hit.text,
                })
                .collect())
        })
    }
}
