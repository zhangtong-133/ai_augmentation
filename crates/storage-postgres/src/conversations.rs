use crate::{PostgresStore, map_error};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    conversations::{Conversation, ConversationStore},
};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

const FIELDS: &str = "id,title,(extract(epoch FROM created_at)*1000)::bigint AS created_ms";
fn record(row: &PgRow) -> Conversation {
    Conversation {
        id: row.get::<Uuid, _>("id").to_string(),
        title: row.get("title"),
        created_at_unix_ms: row.get("created_ms"),
    }
}
fn uuid(value: &str) -> StorageResult<Uuid> {
    Uuid::parse_str(value).map_err(|_| StorageError::InvalidData("invalid conversation id".into()))
}
impl ConversationStore for PostgresStore {
    fn create_conversation(
        &self,
        owner: &UserId,
        request_id: &str,
        title: &str,
    ) -> BoxFuture<'_, StorageResult<Conversation>> {
        let ids = uuid(owner.as_str()).and_then(|owner| Ok((owner, uuid(request_id)?)));
        let title = title.to_owned();
        Box::pin(async move {
            if title.trim().is_empty() || title.chars().count() > 80 || title.contains('\0') {
                return Err(StorageError::InvalidData(
                    "invalid conversation title".into(),
                ));
            }
            let (owner, request_id) = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let result = async {
                sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").execute(&mut *tx).await.map_err(map_error)?;
                // 与删除共用用户锁，串行化幂等检查、清理和配额计算。
                sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE").bind(owner).fetch_one(&mut *tx).await.map_err(map_error)?;
                sqlx::query("DELETE FROM conversations WHERE user_id=$1 AND deleted_at < clock_timestamp()-interval '24 hours'").bind(owner).execute(&mut *tx).await.map_err(map_error)?;
                if let Some(row) = sqlx::query(&format!("SELECT {FIELDS},deleted_at IS NOT NULL AS deleted FROM conversations WHERE user_id=$1 AND request_id=$2"))
                    .bind(owner).bind(request_id).fetch_optional(&mut *tx).await.map_err(map_error)? {
                    if row.get::<bool, _>("deleted") || row.get::<String, _>("title") != title {
                        return Err(StorageError::Conflict("conversation request already used".into()));
                    }
                    return Ok(record(&row));
                }
                let row = sqlx::query("SELECT count(*) FILTER (WHERE deleted_at IS NULL) AS active, count(*) FILTER (WHERE created_at > clock_timestamp()-interval '24 hours') AS daily FROM conversations WHERE user_id=$1")
                    .bind(owner).fetch_one(&mut *tx).await.map_err(map_error)?;
                if row.get::<i64, _>("active") >= 100 || row.get::<i64, _>("daily") >= 100 {
                    return Err(StorageError::Conflict("conversation quota reached".into()));
                }
                let row = sqlx::query(&format!("INSERT INTO conversations(id,user_id,request_id,title) VALUES($1,$2,$3,$4) RETURNING {FIELDS}"))
                    .bind(Uuid::new_v4()).bind(owner).bind(request_id).bind(title).fetch_one(&mut *tx).await.map_err(map_error)?;
                Ok(record(&row))
            }.await;
            if result.is_ok() {
                tx.commit().await.map_err(map_error)?;
            } else {
                tx.rollback().await.map_err(map_error)?;
            }
            result
        })
    }
    fn list_conversations(
        &self,
        owner: &UserId,
    ) -> BoxFuture<'_, StorageResult<Vec<Conversation>>> {
        let owner = uuid(owner.as_str());
        Box::pin(async move {
            let rows = sqlx::query(&format!("SELECT {FIELDS} FROM conversations WHERE user_id=$1 AND deleted_at IS NULL ORDER BY created_at DESC,id DESC LIMIT 100"))
                .bind(owner?).fetch_all(&self.pool).await.map_err(map_error)?;
            Ok(rows.iter().map(record).collect())
        })
    }
    fn get_conversation(
        &self,
        owner: &UserId,
        id: &str,
    ) -> BoxFuture<'_, StorageResult<Conversation>> {
        let ids = uuid(owner.as_str()).and_then(|owner| Ok((owner, uuid(id)?)));
        Box::pin(async move {
            let (owner, id) = ids?;
            let row = sqlx::query(&format!("SELECT {FIELDS} FROM conversations WHERE user_id=$1 AND id=$2 AND deleted_at IS NULL"))
                .bind(owner).bind(id).fetch_one(&self.pool).await.map_err(map_error)?;
            Ok(record(&row))
        })
    }
    fn delete_conversation(&self, owner: &UserId, id: &str) -> BoxFuture<'_, StorageResult<()>> {
        let ids = uuid(owner.as_str()).and_then(|owner| Ok((owner, uuid(id)?)));
        Box::pin(async move {
            let (owner, id) = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let result = async {
                sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE").bind(owner).fetch_one(&mut *tx).await.map_err(map_error)?;
                // 删除即清空标题；保留短期墓碑，避免重试复活，重复删除不延长保留期。
                let changed = sqlx::query("UPDATE conversations SET title='deleted',message_revision=message_revision+CASE WHEN deleted_at IS NULL THEN 1 ELSE 0 END,cache_delete_pending=true,deleted_at=COALESCE(deleted_at,clock_timestamp()) WHERE user_id=$1 AND id=$2")
                    .bind(owner).bind(id).execute(&mut *tx).await.map_err(map_error)?;
                if changed.rows_affected() == 0 { return Err(StorageError::NotFound); }
                sqlx::query("DELETE FROM conversation_messages WHERE conversation_id=$1").bind(id).execute(&mut *tx).await.map_err(map_error)?;
                Ok(())
            }.await;
            if result.is_ok() {
                tx.commit().await.map_err(map_error)?;
            } else {
                tx.rollback().await.map_err(map_error)?;
            }
            result
        })
    }
}
