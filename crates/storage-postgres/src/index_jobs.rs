use crate::{PostgresStore, map_error};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    index_jobs::{IndexFailure, IndexJob, IndexJobStore, IndexLease},
};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

fn id(value: &str) -> StorageResult<Uuid> {
    Uuid::parse_str(value).map_err(|_| StorageError::InvalidData("invalid id".into()))
}
fn profile(value: &str) -> StorageResult<String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(StorageError::InvalidData("invalid index profile".into()));
    }
    Ok(value.into())
}
fn job(row: &PgRow) -> IndexJob {
    IndexJob {
        id: row.get::<Uuid, _>("id").to_string(),
        document_id: row.get::<Uuid, _>("document_id").to_string(),
        profile: row.get("profile"),
        status: row.get("status"),
        indexed_chunks: row.get("indexed_chunks"),
        total_chunks: row.get("total_chunks"),
        attempts: row.get("attempts"),
        error_code: row.get("error_code"),
    }
}
fn changed(rows: u64) -> StorageResult<()> {
    if rows == 1 {
        Ok(())
    } else {
        Err(StorageError::Conflict("index lease lost".into()))
    }
}
impl IndexJobStore for PostgresStore {
    fn enqueue_index(
        &self,
        owner: &UserId,
        document_id: &str,
        target: &str,
    ) -> BoxFuture<'_, StorageResult<IndexJob>> {
        let owner = id(owner.as_str());
        let document_id = id(document_id);
        let target = profile(target);
        Box::pin(async move {
            // 单条语句完成所有权检查、幂等创建和失败任务恢复；不可重置运行中的租约。
            let row = sqlx::query("INSERT INTO document_index_jobs (id,user_id,document_id,profile,total_chunks)
                SELECT $1,user_id,id,$4,cardinality(chunks) FROM documents WHERE user_id=$2 AND id=$3
                ON CONFLICT (user_id,document_id,profile) DO UPDATE SET
                    status=CASE WHEN document_index_jobs.status='failed' THEN 'queued' ELSE document_index_jobs.status END,
                    attempts=CASE WHEN document_index_jobs.status='failed' THEN 0 ELSE document_index_jobs.attempts END,
                    error_code=CASE WHEN document_index_jobs.status='failed' THEN NULL ELSE document_index_jobs.error_code END,
                    available_at=CASE WHEN document_index_jobs.status='failed' THEN NOW() ELSE document_index_jobs.available_at END
                RETURNING *")
                .bind(Uuid::new_v4()).bind(owner?).bind(document_id?).bind(target?).fetch_one(&self.pool).await.map_err(map_error)?;
            Ok(job(&row))
        })
    }
    fn index_status(
        &self,
        owner: &UserId,
        document_id: &str,
        target: &str,
    ) -> BoxFuture<'_, StorageResult<Option<IndexJob>>> {
        let owner = id(owner.as_str());
        let document_id = id(document_id);
        let target = profile(target);
        Box::pin(async move {
            let row = sqlx::query("SELECT j.* FROM documents d LEFT JOIN document_index_jobs j ON j.document_id=d.id AND j.user_id=d.user_id AND j.profile=$3 WHERE d.user_id=$1 AND d.id=$2")
                .bind(owner?).bind(document_id?).bind(target?).fetch_one(&self.pool).await.map_err(map_error)?;
            Ok(row.get::<Option<Uuid>, _>("id").map(|_| job(&row)))
        })
    }
    fn claim_index(&self, target: &str) -> BoxFuture<'_, StorageResult<Option<IndexLease>>> {
        let target = profile(target);
        Box::pin(async move {
            let target = target?;
            // 租约用尽也必须进入终态，避免进程反复退出造成无限付费重试。
            sqlx::query("UPDATE document_index_jobs SET status='failed',lease_until=NULL,lease_token=NULL,error_code='lease_expired',updated_at=NOW() WHERE profile=$1 AND status='running' AND lease_until<=NOW() AND attempts>=5")
                .bind(&target).execute(&self.pool).await.map_err(map_error)?;
            let row = sqlx::query("WITH candidate AS (
                SELECT id FROM document_index_jobs WHERE profile=$1 AND attempts<5
                AND ((status='queued' AND available_at<=NOW()) OR (status='running' AND lease_until<=NOW()))
                ORDER BY available_at,id LIMIT 1 FOR UPDATE SKIP LOCKED)
                UPDATE document_index_jobs j SET status='running',attempts=j.attempts+1,
                    lease_token=$2,lease_until=NOW()+INTERVAL '90 seconds',updated_at=NOW()
                FROM candidate c WHERE j.id=c.id RETURNING j.*")
                .bind(target).bind(Uuid::new_v4()).fetch_optional(&self.pool).await.map_err(map_error)?;
            Ok(row.map(|row| IndexLease {
                job: job(&row),
                owner: UserId::new(row.get::<Uuid, _>("user_id").to_string()),
                token: row.get::<Uuid, _>("lease_token").to_string(),
            }))
        })
    }
    fn complete_index_batch(
        &self,
        lease: &IndexLease,
        indexed_chunks: i32,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let job_id = id(&lease.job.id);
        let token = id(&lease.token);
        Box::pin(async move {
            let result = sqlx::query("UPDATE document_index_jobs SET indexed_chunks=$3,
                status=CASE WHEN $3=total_chunks THEN 'succeeded' ELSE 'queued' END,
                attempts=0,error_code=NULL,lease_token=NULL,lease_until=NULL,available_at=NOW(),updated_at=NOW()
                WHERE id=$1 AND lease_token=$2 AND status='running' AND lease_until>NOW()
                AND $3=LEAST(indexed_chunks+16,total_chunks)")
                .bind(job_id?).bind(token?).bind(indexed_chunks).execute(&self.pool).await.map_err(map_error)?;
            changed(result.rows_affected())
        })
    }
    fn fail_index_batch(
        &self,
        lease: &IndexLease,
        failure: IndexFailure,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let job_id = id(&lease.job.id);
        let token = id(&lease.token);
        Box::pin(async move {
            let result = sqlx::query(
                "UPDATE document_index_jobs SET
                status=CASE WHEN $3 AND attempts<5 THEN 'queued' ELSE 'failed' END,
                error_code=$4,available_at=NOW()+make_interval(secs=>5*power(2,attempts-1)),
                lease_token=NULL,lease_until=NULL,updated_at=NOW()
                WHERE id=$1 AND lease_token=$2 AND status='running' AND lease_until>NOW()",
            )
            .bind(job_id?)
            .bind(token?)
            .bind(failure.retryable())
            .bind(failure.code())
            .execute(&self.pool)
            .await
            .map_err(map_error)?;
            changed(result.rows_affected())
        })
    }
}
