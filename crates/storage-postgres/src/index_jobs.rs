use crate::{PostgresStore, map_error};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    index_jobs::{BatchOutcome, IndexJob, IndexJobStore},
};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

fn job(row: &PgRow) -> IndexJob {
    IndexJob {
        document_id: row.get::<Uuid, _>("document_id").to_string(),
        status: row.get("status"),
        indexed_chunks: row.get("indexed_chunks"),
        total_chunks: row.get("total_chunks"),
        attempts: row.get("attempts"),
        error_code: row.get("error_code"),
        owner: row.get::<Uuid, _>("user_id").to_string(),
        lease: row
            .get::<Option<Uuid>, _>("lease")
            .map_or_else(String::new, |v| v.to_string()),
    }
}
fn id(value: &str) -> StorageResult<Uuid> {
    Uuid::parse_str(value).map_err(|_| StorageError::InvalidData("invalid id".into()))
}
impl IndexJobStore for PostgresStore {
    fn enqueue(
        &self,
        owner: &UserId,
        document: &str,
        target: &str,
    ) -> BoxFuture<'_, StorageResult<IndexJob>> {
        let (owner, document, target) = (id(owner.as_str()), id(document), target.to_owned());
        Box::pin(async move {
            let (owner, document) = (owner?, document?);
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            // 锁定文档，使同一文档的并发入队串行化；所有权检查不读取外部原文。
            let row = sqlx::query("SELECT cardinality(chunks) AS total FROM documents WHERE id=$1 AND user_id=$2 FOR UPDATE")
                .bind(document).bind(owner).fetch_one(&mut *tx).await.map_err(map_error)?;
            let row = sqlx::query("INSERT INTO document_index_jobs(document_id,target,status,total_chunks) VALUES($1,$2,'queued',$3) ON CONFLICT(document_id,target) DO UPDATE SET status=CASE WHEN document_index_jobs.status='failed' THEN 'queued' ELSE document_index_jobs.status END, attempts=CASE WHEN document_index_jobs.status='failed' THEN 0 ELSE document_index_jobs.attempts END, error_code=CASE WHEN document_index_jobs.status='failed' THEN NULL ELSE document_index_jobs.error_code END, available_at=CASE WHEN document_index_jobs.status='failed' THEN NOW() ELSE document_index_jobs.available_at END RETURNING *, $4::uuid AS user_id")
                .bind(document).bind(target).bind(row.get::<i32,_>("total")).bind(owner)
                .fetch_one(&mut *tx).await.map_err(map_error)?;
            tx.commit().await.map_err(map_error)?;
            Ok(job(&row))
        })
    }
    fn index_status(
        &self,
        owner: &UserId,
        document: &str,
        target: &str,
    ) -> BoxFuture<'_, StorageResult<IndexJob>> {
        let (owner, document, target) = (id(owner.as_str()), id(document), target.to_owned());
        Box::pin(async move {
            let row = sqlx::query("SELECT j.*,d.user_id FROM document_index_jobs j JOIN documents d ON d.id=j.document_id WHERE d.id=$1 AND d.user_id=$2 AND j.target=$3")
                .bind(document?).bind(owner?).bind(target).fetch_one(&self.pool).await.map_err(map_error)?;
            Ok(job(&row))
        })
    }
    fn claim(&self, target: &str) -> BoxFuture<'_, StorageResult<Option<IndexJob>>> {
        let target = target.to_owned();
        Box::pin(async move {
            // 崩溃也消耗尝试次数，避免反复崩溃造成无限模型费用。
            sqlx::query("UPDATE document_index_jobs SET status='failed',error_code='lease_expired',lease=NULL,lease_until=NULL WHERE target=$1 AND status='running' AND lease_until<=NOW() AND attempts>=3")
                .bind(&target).execute(&self.pool).await.map_err(map_error)?;
            let row = sqlx::query("WITH candidate AS (SELECT document_id,target FROM document_index_jobs WHERE target=$1 AND attempts<3 AND ((status IN ('queued','retrying') AND available_at<=NOW()) OR (status='running' AND lease_until<=NOW())) ORDER BY available_at,document_id FOR UPDATE SKIP LOCKED LIMIT 1), claimed AS (UPDATE document_index_jobs j SET status='running',attempts=j.attempts+1,lease=$2,lease_until=NOW()+INTERVAL '60 seconds' FROM candidate c WHERE j.document_id=c.document_id AND j.target=c.target RETURNING j.*) SELECT claimed.*,d.user_id FROM claimed JOIN documents d ON d.id=claimed.document_id")
                .bind(target).bind(Uuid::new_v4()).fetch_optional(&self.pool).await.map_err(map_error)?;
            Ok(row.as_ref().map(job))
        })
    }
    fn finish_batch(
        &self,
        target: &str,
        claimed: &IndexJob,
        outcome: BatchOutcome,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let (target, claimed) = (target.to_owned(), claimed.clone());
        Box::pin(async move {
            let (status, progress, attempts, error, delay) = match outcome {
                BatchOutcome::Success(end)
                    if end > claimed.indexed_chunks && end <= claimed.total_chunks =>
                {
                    (
                        if end == claimed.total_chunks {
                            "completed"
                        } else {
                            "queued"
                        },
                        end,
                        0,
                        None,
                        0,
                    )
                }
                BatchOutcome::Success(_) => {
                    return Err(StorageError::InvalidData("invalid progress".into()));
                }
                BatchOutcome::Retry(code) if claimed.attempts < 3 => (
                    "retrying",
                    claimed.indexed_chunks,
                    claimed.attempts,
                    Some(code),
                    if claimed.attempts == 1 { 5 } else { 20 },
                ),
                BatchOutcome::Retry(code) | BatchOutcome::Failed(code) => (
                    "failed",
                    claimed.indexed_chunks,
                    claimed.attempts,
                    Some(code),
                    0,
                ),
            };
            let result = sqlx::query("UPDATE document_index_jobs SET status=$4,indexed_chunks=$5,attempts=$6,error_code=$7,available_at=NOW()+make_interval(secs => $8),lease=NULL,lease_until=NULL WHERE document_id=$1 AND target=$2 AND lease=$3 AND status='running' AND lease_until>NOW()")
                .bind(id(&claimed.document_id)?).bind(target).bind(id(&claimed.lease)?).bind(status).bind(progress).bind(attempts).bind(error).bind(f64::from(delay))
                .execute(&self.pool).await.map_err(map_error)?;
            if result.rows_affected() == 0 {
                return Err(StorageError::Conflict("index lease lost".into()));
            }
            Ok(())
        })
    }
}
