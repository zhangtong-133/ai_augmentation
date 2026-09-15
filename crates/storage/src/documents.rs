use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;
use serde::Serialize;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DocumentStats {
    pub total_documents: i64,
    pub total_chunks: i64,
    pub imported_today: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DocumentSummary {
    pub id: String,
    pub title: String,
    pub source: String,
    pub source_type: String,
    pub tags: Vec<String>,
    pub created_at_unix_ms: i64,
    pub chunk_count: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StoredDocument {
    #[serde(flatten)]
    pub summary: DocumentSummary,
    /// Markdown source or extracted PDF text, depending on `source_type`.
    pub markdown: String,
    #[serde(skip_serializing)]
    pub original_pdf: Option<Vec<u8>>,
    pub chunks: Vec<String>,
}

/// Every operation requires the authenticated owner; no global lookup is exposed.
pub trait DocumentStore: Send + Sync {
    /// Count this owner's documents, with today's imports in `[start_ms, end_ms)`.
    fn document_stats(
        &self,
        owner: &UserId,
        start_ms: i64,
        end_ms: i64,
    ) -> BoxFuture<'_, StorageResult<DocumentStats>>;
    fn insert_document(
        &self,
        owner: &UserId,
        digest: &str,
        document: &StoredDocument,
    ) -> BoxFuture<'_, StorageResult<()>>;
    fn list_documents(
        &self,
        owner: &UserId,
        offset: u32,
    ) -> BoxFuture<'_, StorageResult<Vec<DocumentSummary>>>;
    fn get_document(
        &self,
        owner: &UserId,
        id: &str,
    ) -> BoxFuture<'_, StorageResult<StoredDocument>>;
}
