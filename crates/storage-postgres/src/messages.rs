use crate::{PostgresStore, map_error};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    messages::{CacheDeletion, Message, MessageSnapshot, MessageStore, validate_content},
};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

const FIELDS: &str =
    "id,sequence,content,(extract(epoch FROM created_at)*1000)::bigint AS created_ms";
fn message(row: &PgRow) -> Message {
    Message {
        id: row.get::<Uuid, _>("id").to_string(),
        sequence: row.get("sequence"),
        content: row.get("content"),
        created_at_unix_ms: row.get("created_ms"),
    }
}
fn uuid(value: &str) -> StorageResult<Uuid> {
    Uuid::parse_str(value).map_err(|_| StorageError::InvalidData("invalid message id".into()))
}
impl MessageStore for PostgresStore {
    fn append_message(
        &self,
        owner: &UserId,
        conversation: &str,
        request_id: &str,
        content: &str,
    ) -> BoxFuture<'_, StorageResult<Message>> {
        let ids = uuid(owner.as_str())
            .and_then(|owner| Ok((owner, uuid(conversation)?, uuid(request_id)?)));
        let content = content.to_owned();
        Box::pin(async move {
            validate_content(&content)?;
            let (owner, conversation, request) = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let result = async {
                sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").execute(&mut *tx).await.map_err(map_error)?;
                // 对话行锁与删除 UPDATE 互斥，检查归属后直到提交都持有锁。
                let revision: i64 = sqlx::query_scalar("SELECT message_revision FROM conversations WHERE user_id=$1 AND id=$2 AND deleted_at IS NULL FOR UPDATE")
                    .bind(owner).bind(conversation).fetch_one(&mut *tx).await.map_err(map_error)?;
                if let Some(row) = sqlx::query(&format!("SELECT {FIELDS} FROM conversation_messages WHERE conversation_id=$1 AND request_id=$2"))
                    .bind(conversation).bind(request).fetch_optional(&mut *tx).await.map_err(map_error)? {
                    if row.get::<String, _>("content") != content { return Err(StorageError::Conflict("message request already used".into())); }
                    return Ok(message(&row));
                }
                if revision >= 100 { return Err(StorageError::Conflict("conversation message quota reached".into())); }
                let row = sqlx::query(&format!("INSERT INTO conversation_messages(id,conversation_id,request_id,sequence,content) VALUES($1,$2,$3,$4,$5) RETURNING {FIELDS}"))
                    .bind(Uuid::new_v4()).bind(conversation).bind(request).bind(revision+1).bind(content).fetch_one(&mut *tx).await.map_err(map_error)?;
                sqlx::query("UPDATE conversations SET message_revision=message_revision+1 WHERE id=$1").bind(conversation).execute(&mut *tx).await.map_err(map_error)?;
                Ok(message(&row))
            }.await;
            if result.is_ok() {
                tx.commit().await.map_err(map_error)?;
            } else {
                tx.rollback().await.map_err(map_error)?;
            }
            result
        })
    }
    fn message_revision(
        &self,
        owner: &UserId,
        conversation: &str,
    ) -> BoxFuture<'_, StorageResult<i64>> {
        let ids = uuid(owner.as_str()).and_then(|owner| Ok((owner, uuid(conversation)?)));
        Box::pin(async move {
            let (owner, conversation) = ids?;
            sqlx::query_scalar("SELECT message_revision FROM conversations WHERE user_id=$1 AND id=$2 AND deleted_at IS NULL")
                .bind(owner).bind(conversation).fetch_one(&self.pool).await.map_err(map_error)
        })
    }
    fn message_snapshot(
        &self,
        owner: &UserId,
        conversation: &str,
    ) -> BoxFuture<'_, StorageResult<MessageSnapshot>> {
        let ids = uuid(owner.as_str()).and_then(|owner| Ok((owner, uuid(conversation)?)));
        Box::pin(async move {
            let (owner, conversation) = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let result = async {
                // 共享锁保证版本和消息属于同一个快照，不与追加/删除交错。
                let revision = sqlx::query_scalar("SELECT message_revision FROM conversations WHERE user_id=$1 AND id=$2 AND deleted_at IS NULL FOR SHARE")
                    .bind(owner).bind(conversation).fetch_one(&mut *tx).await.map_err(map_error)?;
                let rows = sqlx::query(&format!("SELECT {FIELDS} FROM conversation_messages WHERE conversation_id=$1 ORDER BY sequence LIMIT 100"))
                    .bind(conversation).fetch_all(&mut *tx).await.map_err(map_error)?;
                Ok(MessageSnapshot { revision, deleted: false, messages: rows.iter().map(message).collect() })
            }.await;
            if result.is_ok() {
                tx.commit().await.map_err(map_error)?;
            } else {
                tx.rollback().await.map_err(map_error)?;
            }
            result
        })
    }
    fn pending_cache_deletions(&self) -> BoxFuture<'_, StorageResult<Vec<CacheDeletion>>> {
        Box::pin(async {
            let rows = sqlx::query("SELECT user_id,id,message_revision FROM conversations WHERE cache_delete_pending AND deleted_at IS NOT NULL ORDER BY deleted_at LIMIT 20")
                .fetch_all(&self.pool).await.map_err(map_error)?;
            Ok(rows
                .iter()
                .map(|row| CacheDeletion {
                    owner: UserId::new(row.get::<Uuid, _>("user_id").to_string()),
                    conversation: row.get::<Uuid, _>("id").to_string(),
                    revision: row.get("message_revision"),
                })
                .collect())
        })
    }
    fn acknowledge_cache_deletion(&self, item: &CacheDeletion) -> BoxFuture<'_, StorageResult<()>> {
        let ids =
            uuid(item.owner.as_str()).and_then(|owner| Ok((owner, uuid(&item.conversation)?)));
        let revision = item.revision;
        Box::pin(async move {
            let (owner, conversation) = ids?;
            sqlx::query("UPDATE conversations SET cache_delete_pending=false WHERE user_id=$1 AND id=$2 AND deleted_at IS NOT NULL AND message_revision=$3")
                .bind(owner).bind(conversation).bind(revision).execute(&self.pool).await.map_err(map_error)?;
            Ok(())
        })
    }
}
