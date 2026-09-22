#![forbid(unsafe_code)]

use personal_ai_domain::{ConversationId, UserId};
use personal_ai_storage::{BoxFuture, MemoryEntry, MemoryStore, StorageError, StorageResult};
use sha2::{Digest, Sha256};
use std::{future::Future, time::Duration};

/// 可信服务端使用的短期对话存储；调用方负责认证及对话授权。
#[derive(Clone)]
pub struct RedisMemoryStore {
    client: redis::Client,
    ttl_seconds: u32,
    capacity: usize,
}

fn invalid() -> StorageError {
    StorageError::InvalidData("invalid short memory input".into())
}
fn unavailable() -> StorageError {
    // 不向调用方泄露 Redis URL、凭据或底层错误正文。
    StorageError::Unavailable("short memory unavailable".into())
}
async fn bounded<T>(future: impl Future<Output = StorageResult<T>>) -> StorageResult<T> {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .map_err(|_| unavailable())?
}

fn keys(owner: &UserId, conversation: &ConversationId) -> StorageResult<(String, String)> {
    for value in [owner.as_str(), conversation.as_str()] {
        if value.trim().is_empty() || value.len() > 128 || value.contains('\0') {
            return Err(invalid());
        }
    }
    // 固定长度摘要避免分隔符碰撞；同一用户的键使用同一个 Redis hash tag。
    let owner = format!("{:x}", Sha256::digest(owner.as_str().as_bytes()));
    let conversation = format!("{:x}", Sha256::digest(conversation.as_str().as_bytes()));
    Ok((
        format!("memory:v1:{{{owner}}}:{conversation}"),
        format!("memory:v1:{{{owner}}}:active"),
    ))
}
fn encode(entry: &MemoryEntry) -> StorageResult<String> {
    if entry.key.trim().is_empty()
        || entry.key.len() > 128
        || entry.value.trim().is_empty()
        || entry.value.len() > 4096
        || entry.key.contains('\0')
        || entry.value.contains('\0')
    {
        return Err(invalid());
    }
    let encoded = serde_json::to_string(entry).map_err(|_| invalid())?;
    if encoded.len() > 8192 {
        return Err(invalid());
    }
    Ok(encoded)
}

impl RedisMemoryStore {
    /// TTL 为 1–86400 秒，每对话保留 1–100 条；每用户最多 32 个活跃对话。
    ///
    /// # Errors
    /// URL 或容量参数无效时返回错误；连接在每次操作时建立，不自动重试写入。
    pub fn new(url: &str, ttl_seconds: u32, capacity: usize) -> StorageResult<Self> {
        if !(1..=86400).contains(&ttl_seconds) || !(1..=100).contains(&capacity) {
            return Err(invalid());
        }
        Ok(Self {
            client: redis::Client::open(url).map_err(|_| invalid())?,
            ttl_seconds,
            capacity,
        })
    }

    /// 清空指定用户的对话并释放名额；不存在时同样成功。
    ///
    /// # Errors
    /// 身份参数无效或 Redis 不可用时返回错误。
    pub async fn clear(&self, owner: &UserId, conversation: &ConversationId) -> StorageResult<()> {
        let (key, index) = keys(owner, conversation)?;
        bounded(async {
            let mut connection = self
                .client
                .get_multiplexed_async_connection()
                .await
                .map_err(|_| unavailable())?;
            redis::Script::new(
                "redis.call('ZREM', KEYS[2], KEYS[1]); return redis.call('DEL', KEYS[1])",
            )
            .key(key)
            .key(index)
            .invoke_async::<i64>(&mut connection)
            .await
            .map_err(|_| unavailable())?;
            Ok(())
        })
        .await
    }
}

impl MemoryStore for RedisMemoryStore {
    fn append(
        &self,
        owner: &UserId,
        conversation: &ConversationId,
        entry: &MemoryEntry,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let input = keys(owner, conversation).and_then(|keys| Ok((keys, encode(entry)?)));
        Box::pin(bounded(async move {
            let ((key, index), encoded) = input?;
            let mut connection = self
                .client
                .get_multiplexed_async_connection()
                .await
                .map_err(|_| unavailable())?;
            let accepted: i64 = redis::Script::new(include_str!("append.lua"))
                .key(key)
                .key(index)
                .arg(encoded)
                .arg(self.capacity)
                .arg(u64::from(self.ttl_seconds) * 1000)
                .invoke_async(&mut connection)
                .await
                .map_err(|_| unavailable())?;
            if accepted != 1 {
                return Err(StorageError::Conflict(
                    "active conversation quota reached".into(),
                ));
            }
            Ok(())
        }))
    }

    fn recent(
        &self,
        owner: &UserId,
        conversation: &ConversationId,
        limit: usize,
    ) -> BoxFuture<'_, StorageResult<Vec<MemoryEntry>>> {
        let input = keys(owner, conversation);
        Box::pin(bounded(async move {
            let (key, _) = input?;
            if limit == 0 || limit > self.capacity {
                return Err(invalid());
            }
            let mut connection = self
                .client
                .get_multiplexed_async_connection()
                .await
                .map_err(|_| unavailable())?;
            let rows: Vec<String> = redis::cmd("LRANGE")
                .arg(key)
                .arg(-i64::try_from(limit).map_err(|_| invalid())?)
                .arg(-1)
                .query_async(&mut connection)
                .await
                .map_err(|_| unavailable())?;
            rows.into_iter()
                .map(|row| {
                    if row.len() > 8192 {
                        return Err(invalid());
                    }
                    let entry: MemoryEntry = serde_json::from_str(&row).map_err(|_| invalid())?;
                    encode(&entry)?;
                    Ok(entry)
                })
                .collect()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_and_collision_safe_keys() {
        for (ttl, capacity) in [(0, 1), (86401, 1), (1, 0), (1, 101)] {
            assert!(RedisMemoryStore::new("redis://127.0.0.1/", ttl, capacity).is_err());
        }
        assert_ne!(
            keys(&UserId::new("a:b"), &ConversationId::new("c")).unwrap(),
            keys(&UserId::new("a"), &ConversationId::new("b:c")).unwrap()
        );
        assert!(keys(&UserId::new(""), &ConversationId::new("c")).is_err());
        for value in [
            String::new(),
            "x".repeat(4097),
            "\0".into(),
            "\n".repeat(4096),
            "\u{1}".repeat(4096),
        ] {
            assert!(
                encode(&MemoryEntry {
                    key: "entry".into(),
                    value,
                    created_at_unix_ms: 0
                })
                .is_err()
            );
        }
    }
}
