use crate::{BoxFuture, StorageError, StorageResult};
use personal_ai_domain::UserId;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub id: String,
    pub sequence: i64,
    pub content: String,
    pub created_at_unix_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MessageSnapshot {
    pub revision: i64,
    pub deleted: bool,
    pub messages: Vec<Message>,
}

pub struct CacheDeletion {
    pub owner: UserId,
    pub conversation: String,
    pub revision: i64,
}

/// 本阶段仅接收用户消息，不接收模型角色或客户端时间戳。
///
/// # Errors
/// 空白、NUL 或超过 4096 字节时返回参数错误。
pub fn validate_content(content: &str) -> StorageResult<()> {
    if content.trim().is_empty() || content.len() > 4096 || content.contains('\0') {
        return Err(StorageError::InvalidData("invalid message content".into()));
    }
    Ok(())
}

pub trait MessageStore: Send + Sync {
    fn append_message(
        &self,
        owner: &UserId,
        conversation: &str,
        request_id: &str,
        content: &str,
    ) -> BoxFuture<'_, StorageResult<Message>>;
    fn message_revision(
        &self,
        owner: &UserId,
        conversation: &str,
    ) -> BoxFuture<'_, StorageResult<i64>>;
    fn message_snapshot(
        &self,
        owner: &UserId,
        conversation: &str,
    ) -> BoxFuture<'_, StorageResult<MessageSnapshot>>;
    fn pending_cache_deletions(&self) -> BoxFuture<'_, StorageResult<Vec<CacheDeletion>>>;
    fn acknowledge_cache_deletion(&self, item: &CacheDeletion) -> BoxFuture<'_, StorageResult<()>>;
}

pub trait MessageCache: Send + Sync {
    fn get(
        &self,
        owner: &UserId,
        conversation: &str,
    ) -> BoxFuture<'_, StorageResult<Option<MessageSnapshot>>>;
    fn put(
        &self,
        owner: &UserId,
        conversation: &str,
        snapshot: &MessageSnapshot,
    ) -> BoxFuture<'_, StorageResult<()>>;
}
