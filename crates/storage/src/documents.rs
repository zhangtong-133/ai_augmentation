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
    /// 根据 `source_type` 保存 Markdown 原文，或 PDF/网页提取文本。
    pub markdown: String,
    #[serde(skip_serializing)]
    pub original_pdf: Option<Vec<u8>>,
    #[serde(skip_serializing)]
    pub original_html: Option<String>,
    pub chunks: Vec<String>,
}

/// 每项操作都要求提供已认证的所有者，不暴露全局查询接口。
pub trait DocumentStore: Send + Sync {
    /// 统计该所有者的文档；今日导入量采用 `[start_ms, end_ms)` 时间区间。
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
