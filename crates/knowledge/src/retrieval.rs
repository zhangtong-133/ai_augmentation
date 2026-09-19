use crate::index::{DocumentIndexer, IndexError};
use personal_ai_domain::UserId;
use personal_ai_llm::LlmError;
use personal_ai_storage::{StorageError, documents::DocumentStore};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub struct SearchHit {
    pub document_id: String,
    pub title: String,
    pub source: String,
    pub ordinal: usize,
    pub text: String,
    pub score: f32,
}

impl DocumentIndexer {
    /// 召回结果必须由当前所有者的权威文本复核；向量载荷不是权限依据。
    ///
    /// # Errors
    /// 参数、模型或存储异常时失败，避免将依赖故障伪装成空结果。
    pub async fn search(
        &self,
        owner: &UserId,
        documents: &dyn DocumentStore,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>, IndexError> {
        if query.trim().is_empty() || query.chars().count() > 1000 || !(1..=20).contains(&limit) {
            return Err(IndexError::Model(LlmError::InvalidRequest(
                "invalid query".into(),
            )));
        }
        let embeddings = self
            .provider
            .embedding(&[query.trim().into()])
            .await
            .map_err(IndexError::Model)?;
        let [embedding] = embeddings.as_slice() else {
            return Err(IndexError::Model(LlmError::InvalidResponse(
                "invalid query embedding".into(),
            )));
        };
        if embedding.model != self.model
            || embedding.values.len() != self.dimensions
            || embedding.values.iter().any(|v| !v.is_finite())
            || !embedding.values.iter().any(|v| *v != 0.0)
        {
            return Err(IndexError::Model(LlmError::InvalidResponse(
                "invalid query embedding".into(),
            )));
        }
        let matches = self
            .vectors
            .similar_search(owner, &embedding.values, 20)
            .await
            .map_err(IndexError::Storage)?;
        let mut documents_cache = HashMap::new();
        let mut seen = HashSet::new();
        let mut hits = Vec::new();
        for matched in matches.into_iter().take(20) {
            let record = matched.record;
            if !matched.score.is_finite()
                || matched.score < 0.3
                || record.model != self.model
                || Uuid::parse_str(&record.document_id).is_err()
                || record.id != format!("{}:{}", record.document_id, record.ordinal)
            {
                continue;
            }
            if !documents_cache.contains_key(&record.document_id) {
                let document = match documents
                    .get_document_text(owner, &record.document_id)
                    .await
                {
                    Ok(document) => Some(document),
                    Err(StorageError::NotFound) => None,
                    Err(error) => return Err(IndexError::Storage(error)),
                };
                documents_cache.insert(record.document_id.clone(), document);
            }
            let Some(document) = &documents_cache[&record.document_id] else {
                continue;
            };
            if document.chunks.get(record.ordinal) != Some(&record.text)
                || document.summary.id != record.document_id
                || !seen.insert((record.document_id.clone(), record.ordinal))
            {
                continue;
            }
            hits.push(SearchHit {
                document_id: document.summary.id.clone(),
                title: document.summary.title.clone(),
                source: document.summary.source.clone(),
                ordinal: record.ordinal,
                text: document.chunks[record.ordinal].clone(),
                score: matched.score,
            });
            if hits.len() == limit {
                break;
            }
        }
        Ok(hits)
    }
}
