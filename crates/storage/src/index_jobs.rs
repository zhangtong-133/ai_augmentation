use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IndexJob {
    pub id: String,
    pub document_id: String,
    pub profile: String,
    pub status: String,
    pub indexed_chunks: i32,
    pub total_chunks: i32,
    /// 当前批次的尝试次数；成功推进一批后归零。
    pub attempts: i32,
    pub error_code: Option<String>,
}
#[derive(Clone, Debug)]
pub struct IndexLease {
    pub job: IndexJob,
    pub owner: UserId,
    pub token: String,
}
#[derive(Clone, Copy, Debug)]
pub enum IndexFailure {
    ModelUnavailable,
    ModelRateLimited,
    InvalidEmbedding,
    VectorUnavailable,
    DocumentUnavailable,
    InvalidDocument,
    Timeout,
}
impl IndexFailure {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::ModelUnavailable => "embedding_unavailable",
            Self::ModelRateLimited => "embedding_rate_limited",
            Self::InvalidEmbedding => "invalid_embedding",
            Self::VectorUnavailable => "vector_store_unavailable",
            Self::DocumentUnavailable => "document_unavailable",
            Self::InvalidDocument => "invalid_document",
            Self::Timeout => "indexing_timeout",
        }
    }
    #[must_use]
    pub fn retryable(self) -> bool {
        !matches!(self, Self::InvalidEmbedding | Self::InvalidDocument)
    }
}
/// 用户接口要求 owner；领取与结算仅供后台服务使用，不能暴露为 HTTP 路由。
pub trait IndexJobStore: Send + Sync {
    fn enqueue_index(
        &self,
        owner: &UserId,
        document_id: &str,
        profile: &str,
    ) -> BoxFuture<'_, StorageResult<IndexJob>>;
    fn index_status(
        &self,
        owner: &UserId,
        document_id: &str,
        profile: &str,
    ) -> BoxFuture<'_, StorageResult<Option<IndexJob>>>;
    fn claim_index(&self, profile: &str) -> BoxFuture<'_, StorageResult<Option<IndexLease>>>;
    fn complete_index_batch(
        &self,
        lease: &IndexLease,
        indexed_chunks: i32,
    ) -> BoxFuture<'_, StorageResult<()>>;
    fn fail_index_batch(
        &self,
        lease: &IndexLease,
        failure: IndexFailure,
    ) -> BoxFuture<'_, StorageResult<()>>;
}
