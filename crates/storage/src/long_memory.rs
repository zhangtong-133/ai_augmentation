use crate::{BoxFuture, StorageError, StorageResult};
use personal_ai_domain::UserId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryFact {
    pub id: String,
    pub title: String,
    pub content: String,
    pub version: i64,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
}

/// 在 API 和适配器边界共同校验，避免绕过 HTTP 后写入无效数据。
///
/// # Errors
/// 标题或正文为空、含 NUL 或超长时返回错误。
pub fn validate(title: &str, content: &str) -> StorageResult<()> {
    if title.trim().is_empty()
        || title.chars().count() > 80
        || title.contains('\0')
        || content.trim().is_empty()
        || content.chars().count() > 2000
        || content.contains('\0')
    {
        return Err(StorageError::InvalidData("invalid memory fact".into()));
    }
    Ok(())
}

pub trait LongMemoryStore: Send + Sync {
    fn list_facts(
        &self,
        owner: &UserId,
        offset: i64,
    ) -> BoxFuture<'_, StorageResult<Vec<MemoryFact>>>;
    fn create_fact(
        &self,
        owner: &UserId,
        title: &str,
        content: &str,
    ) -> BoxFuture<'_, StorageResult<MemoryFact>>;
    fn update_fact(
        &self,
        owner: &UserId,
        id: &str,
        version: i64,
        title: &str,
        content: &str,
    ) -> BoxFuture<'_, StorageResult<MemoryFact>>;
    fn delete_fact(
        &self,
        owner: &UserId,
        id: &str,
        version: i64,
    ) -> BoxFuture<'_, StorageResult<()>>;
}
