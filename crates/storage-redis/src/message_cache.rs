use crate::{bounded, keys, unavailable};
use personal_ai_domain::{ConversationId, UserId};
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    messages::{MessageCache, MessageSnapshot, validate_content},
};

/// 与旧的追加式短期记忆使用不同命名空间，缓存过期不会删除持久化消息。
pub struct RedisMessageCache {
    client: redis::Client,
}
impl RedisMessageCache {
    /// # Errors
    /// 无效连接 URL 返回脱敏错误；连接失败由每次操作报告。
    pub fn new(url: &str) -> StorageResult<Self> {
        Ok(Self {
            client: redis::Client::open(url).map_err(|_| unavailable())?,
        })
    }
}
fn key(owner: &UserId, conversation: &str) -> StorageResult<String> {
    Ok(keys(owner, &ConversationId::new(conversation))?
        .0
        .replacen("memory:v1:", "messages:v1:", 1))
}
fn validate(snapshot: &MessageSnapshot) -> StorageResult<()> {
    if !(0..=101).contains(&snapshot.revision)
        || snapshot.messages.len() > 100
        || (snapshot.deleted && !snapshot.messages.is_empty())
        || (!snapshot.deleted
            && snapshot.revision != i64::try_from(snapshot.messages.len()).unwrap_or(-1))
    {
        return Err(StorageError::InvalidData("invalid message snapshot".into()));
    }
    for (index, message) in snapshot.messages.iter().enumerate() {
        if usize::try_from(message.sequence).ok() != Some(index + 1) {
            return Err(StorageError::InvalidData("invalid message sequence".into()));
        }
        validate_content(&message.content)?;
    }
    Ok(())
}
impl MessageCache for RedisMessageCache {
    fn get(
        &self,
        owner: &UserId,
        conversation: &str,
    ) -> BoxFuture<'_, StorageResult<Option<MessageSnapshot>>> {
        let key = key(owner, conversation);
        Box::pin(bounded(async move {
            let key = key?;
            let mut connection = self
                .client
                .get_multiplexed_async_connection()
                .await
                .map_err(|_| unavailable())?;
            // 先在 Redis 端限制长度，避免损坏缓存导致无界响应分配。
            let value: Option<String> = redis::Script::new("if redis.call('STRLEN',KEYS[1]) > 3145728 then return redis.error_reply('oversized cache') end; return redis.call('GET',KEYS[1])")
                .key(key).invoke_async(&mut connection).await.map_err(|_| unavailable())?;
            value
                .map(|value| {
                    let snapshot = serde_json::from_str(&value).map_err(|_| unavailable())?;
                    validate(&snapshot)?;
                    Ok(snapshot)
                })
                .transpose()
        }))
    }
    fn put(
        &self,
        owner: &UserId,
        conversation: &str,
        snapshot: &MessageSnapshot,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let input = validate(snapshot).and_then(|()| {
            Ok((
                key(owner, conversation)?,
                serde_json::to_string(snapshot).map_err(|_| unavailable())?,
                snapshot.revision,
            ))
        });
        Box::pin(bounded(async move {
            let (key, value, revision) = input?;
            if value.len() > 3 * 1024 * 1024 {
                return Err(unavailable());
            }
            let mut connection = self
                .client
                .get_multiplexed_async_connection()
                .await
                .map_err(|_| unavailable())?;
            // 旧请求不能覆盖新快照/删除栅栏；同版本重试不续期。
            redis::Script::new(include_str!("message_cache.lua"))
                .key(key)
                .arg(value)
                .arg(revision)
                .invoke_async::<i64>(&mut connection)
                .await
                .map_err(|_| unavailable())?;
            Ok(())
        }))
    }
}
