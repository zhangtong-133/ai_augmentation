use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct IndexJob {
    pub document_id: String,
    pub status: String,
    pub indexed_chunks: i32,
    pub total_chunks: i32,
    pub attempts: i32,
    pub error_code: Option<String>,
    #[serde(skip)]
    pub owner: String,
    #[serde(skip)]
    pub lease: String,
}

/// HTTP 操作必须使用会话所有者；领取和完成接口仅供服务端后台执行器使用。
pub trait IndexJobStore: Send + Sync {
    fn enqueue(
        &self,
        owner: &UserId,
        document: &str,
        target: &str,
    ) -> BoxFuture<'_, StorageResult<IndexJob>>;
    fn index_status(
        &self,
        owner: &UserId,
        document: &str,
        target: &str,
    ) -> BoxFuture<'_, StorageResult<IndexJob>>;
    fn claim(&self, target: &str) -> BoxFuture<'_, StorageResult<Option<IndexJob>>>;
    /// 租约令牌与未过期条件共同防止旧执行器覆盖新进度。
    fn finish_batch(
        &self,
        target: &str,
        job: &IndexJob,
        outcome: BatchOutcome,
    ) -> BoxFuture<'_, StorageResult<()>>;
}

pub enum BatchOutcome {
    Success(i32),
    Retry(&'static str),
    Failed(&'static str),
}
