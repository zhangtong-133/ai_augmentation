use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DocumentSummary {
    pub id: String,
    pub title: String,
    pub source: String,
    pub tags: Vec<String>,
    pub created_at_unix_ms: i64,
    pub chunk_count: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StoredDocument {
    #[serde(flatten)]
    pub summary: DocumentSummary,
    pub markdown: String,
    pub chunks: Vec<String>,
}

/// Every operation requires the authenticated owner; no global lookup is exposed.
pub trait DocumentStore: Send + Sync {
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
