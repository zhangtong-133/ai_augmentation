use crate::{PostgresStore, map_error};
use personal_ai_storage::{StorageError, StorageResult};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

// 新导入、迁移使用共享锁；删除前使用同一个数据库级排他锁。
const ORIGINALS_LOCK: i64 = 7_384_920_617;
// 元数据按秒向下取整，再额外保留一秒，保证不会提前删除。
const RETENTION_MS: i64 = 24 * 60 * 60 * 1000 + 1000;

#[derive(Debug, Default)]
pub struct MaintenanceReport {
    pub scanned: usize,
    pub eligible: usize,
    pub changed: usize,
    pub next_cursor: Option<String>,
}

pub(super) async fn writer_lock(tx: &mut Transaction<'_, Postgres>) -> StorageResult<()> {
    sqlx::query("SET LOCAL lock_timeout = '5s'")
        .execute(&mut **tx)
        .await
        .map_err(map_error)?;
    sqlx::query("SELECT pg_advisory_xact_lock_shared($1)")
        .bind(ORIGINALS_LOCK)
        .execute(&mut **tx)
        .await
        .map_err(map_error)?;
    Ok(())
}

fn managed_key(key: &str) -> bool {
    let parts: Vec<_> = key.split('/').collect();
    parts.len() == 5
        && parts[0] == "users"
        && parts[2] == "documents"
        && [parts[1], parts[3], parts[4]]
            .iter()
            .all(|value| Uuid::parse_str(value).is_ok_and(|id| id.to_string() == *value))
}

// 普通返回路径必须等待事务结束；Drop 只排队回滚，不能保证下一次取锁前已释放。
async fn finish_item(
    tx: Transaction<'_, Postgres>,
    result: StorageResult<bool>,
    apply: bool,
) -> StorageResult<bool> {
    if matches!(result, Ok(true)) && apply {
        tx.commit().await.map_err(map_error)?;
    } else {
        tx.rollback().await.map_err(map_error)?;
    }
    result
}

impl PostgresStore {
    /// 分批迁移旧内联原文；默认调用方应使用预览，不自动执行全库迁移。
    ///
    /// # Errors
    /// 缺少对象存储、数据库/上传/回读校验失败时保留原来的内联数据。
    pub async fn migrate_originals(
        &self,
        apply: bool,
        limit: usize,
    ) -> StorageResult<MaintenanceReport> {
        let objects = self
            .objects
            .as_ref()
            .ok_or_else(|| StorageError::Unavailable("object store is not configured".into()))?;
        let limit = i64::try_from(limit)
            .ok()
            .filter(|v| (1..=100).contains(v))
            .ok_or_else(|| StorageError::InvalidData("limit must be 1..100".into()))?;
        let ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM documents WHERE original_object_key IS NULL ORDER BY id LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(map_error)?;
        let mut report = MaintenanceReport::default();
        for id in ids {
            report.scanned += 1;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let result = async {
            if apply {
                writer_lock(&mut tx).await?;
            }
            let row = sqlx::query("SELECT *,cardinality(chunks) AS chunk_count FROM documents WHERE id=$1 AND original_object_key IS NULL FOR UPDATE SKIP LOCKED")
                .bind(id).fetch_optional(&mut *tx).await.map_err(map_error)?;
            let Some(row) = row else {
                return Ok(false);
            };
            if apply {
                let owner = row.get::<Uuid, _>("user_id");
                let document = super::documents::stored(&row);
                let (bytes, content_type) = super::documents::original(&document)?;
                let key = format!("users/{owner}/documents/{id}/{}", Uuid::new_v4());
                objects.put(&key, bytes, Some(content_type)).await?;
                // 在丢弃 PDF/HTML 内联副本前进行字节级回读；失败不切换数据库引用。
                if objects.get(&key).await? != bytes {
                    return Err(StorageError::Unavailable(
                        "original verification failed".into(),
                    ));
                }
                sqlx::query("UPDATE documents SET original_object_key=$2,original_pdf=NULL,original_html=NULL WHERE id=$1")
                    .bind(id).bind(key).execute(&mut *tx).await.map_err(map_error)?;
            }
            Ok(true)
            }.await;
            if finish_item(tx, result, apply).await? {
                report.eligible += 1;
                report.changed += usize::from(apply);
            }
        }
        Ok(report)
    }

    /// 扫描一页对象，只删除超过 24 小时且再次核对无引用的原文对象。
    /// 所有写入进程必须使用本版本协议，桶必须由当前数据库独占。
    ///
    /// # Errors
    /// 元数据查询、数据库锁或删除失败时立即停止，不跳过校验。
    pub async fn collect_originals(
        &self,
        apply: bool,
        after: &str,
    ) -> StorageResult<MaintenanceReport> {
        let objects = self
            .objects
            .as_ref()
            .ok_or_else(|| StorageError::Unavailable("object store is not configured".into()))?;
        let entries = objects.list_originals(after).await?;
        let mut report = MaintenanceReport {
            next_cursor: entries.last().map(|entry| entry.key.clone()),
            ..MaintenanceReport::default()
        };
        for entry in entries {
            report.scanned += 1;
            if !managed_key(&entry.key) {
                continue;
            }
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let result = async {
            if apply {
                // 不等待进行中的上传；忙碌时明确失败，管理员稍后重试该页。
                // 锁后的引用查询必须使用新快照，且禁止在延迟副本上决定删除。
                sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
                    .execute(&mut *tx)
                    .await
                    .map_err(map_error)?;
                let locked: bool = sqlx::query_scalar("SELECT CASE WHEN pg_is_in_recovery() OR current_setting('transaction_read_only') = 'on' THEN false ELSE pg_try_advisory_xact_lock($1) END")
                    .bind(ORIGINALS_LOCK)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(map_error)?;
                if !locked {
                    return Err(StorageError::Conflict(
                        "cleanup requires writable primary and idle original writers".into(),
                    ));
                }
            }
            let row = sqlx::query("SELECT (EXTRACT(EPOCH FROM clock_timestamp())*1000)::bigint AS now_ms, EXISTS(SELECT 1 FROM documents WHERE original_object_key=$1) AS referenced")
                .bind(&entry.key).fetch_one(&mut *tx).await.map_err(map_error)?;
            let cutoff = row.get::<i64, _>("now_ms") - RETENTION_MS;
            if row.get::<bool, _>("referenced") || entry.modified_unix_ms > cutoff {
                return Ok(false);
            }
            // 列举结果可能陈旧；在写入排他锁下重新查询对象时间，失败则停止。
            let current = match objects.head(&entry.key).await {
                Ok(current) => current,
                Err(StorageError::NotFound) => return Ok(false),
                Err(error) => return Err(error),
            };
            if current.key != entry.key
                || current.modified_unix_ms != entry.modified_unix_ms
                || current.modified_unix_ms > cutoff
            {
                return Ok(false);
            }
            if apply {
                match objects.delete(&entry.key).await {
                    Ok(()) | Err(StorageError::NotFound) => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(true)
            }.await;
            if finish_item(tx, result, apply).await? {
                report.eligible += 1;
                report.changed += usize::from(apply);
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_only_accepts_exact_generated_key_shape() {
        let id = Uuid::new_v4();
        assert!(managed_key(&format!("users/{id}/documents/{id}/{id}")));
        for key in ["users/other", "backup/file", "users/../documents/a/b"] {
            assert!(!managed_key(key));
        }
        assert!(!managed_key(&format!(
            "users/{id}/documents/{id}/{id}/extra"
        )));
    }
}
