use crate::{PostgresStore, map_error, schedules::uuid};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageResult,
    schedule_operations::{
        ScheduleAudit, ScheduleAuditCounts, ScheduleAuditItem, ScheduleOperationsStore,
    },
};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

// 同一份元数据表达式用于全量汇总和当前页；不得加入正文、授权 JSON 或 claim_id。
const AUDIT: &str = r"
WITH audit AS (
 SELECT s.request_id,s.version,
   CASE WHEN s.status='draft' AND s.approval_expires_ms<=$2 THEN 'expired' ELSE s.status END AS status,
   s.run_at_ms,s.created_ms,s.approval_expires_ms,s.approved_ms,s.lease_until_ms,s.delivered_ms,s.cancelled_ms,
   r.delivered_ms AS reminder_ms,r.request_id IS NOT NULL AS reminder_present,
   s.run_at_ms<=$2 AND (s.status='scheduled' OR (s.status='running' AND s.lease_until_ms<=$2)) AS ready,
   array_remove(ARRAY[
     CASE WHEN s.status='delivered' AND r.request_id IS NULL THEN 'missing_reminder' END,
     CASE WHEN s.status<>'delivered' AND r.request_id IS NOT NULL THEN 'unexpected_reminder' END,
     CASE WHEN s.status='delivered' AND r.request_id IS NOT NULL AND s.delivered_ms IS DISTINCT FROM r.delivered_ms THEN 'delivery_time_mismatch' END,
     CASE WHEN s.delivered_ms<s.run_at_ms THEN 'delivery_before_due' END,
     CASE WHEN s.approved_ms IS NOT NULL AND (s.approved_ms<s.created_ms OR s.approved_ms>=s.approval_expires_ms) THEN 'approval_outside_window' END,
     CASE WHEN s.status IN ('scheduled','running','delivered') AND s.approved_ms IS NULL THEN 'missing_approval_time' END,
     CASE WHEN s.status IN ('draft','scheduled','cancelled','failed') AND s.lease_until_ms IS NOT NULL THEN 'unexpected_lease' END
   ],NULL) AS issues
 FROM schedules s LEFT JOIN schedule_reminders r ON r.user_id=s.user_id AND r.request_id=s.request_id
 WHERE s.user_id=$1
)
";
fn item(row: &PgRow) -> ScheduleAuditItem {
    ScheduleAuditItem {
        request_id: row.get::<Uuid, _>("request_id").to_string(),
        version: row.get("version"),
        status: row.get("status"),
        run_at_unix_ms: row.get::<i64, _>("run_at_ms").to_string(),
        created_at_unix_ms: row.get::<i64, _>("created_ms").to_string(),
        approval_expires_at_unix_ms: row.get::<i64, _>("approval_expires_ms").to_string(),
        approved_at_unix_ms: row
            .get::<Option<i64>, _>("approved_ms")
            .map(|v| v.to_string()),
        lease_until_unix_ms: row
            .get::<Option<i64>, _>("lease_until_ms")
            .map(|v| v.to_string()),
        delivered_at_unix_ms: row
            .get::<Option<i64>, _>("delivered_ms")
            .map(|v| v.to_string()),
        cancelled_at_unix_ms: row
            .get::<Option<i64>, _>("cancelled_ms")
            .map(|v| v.to_string()),
        reminder_delivered_at_unix_ms: row
            .get::<Option<i64>, _>("reminder_ms")
            .map(|v| v.to_string()),
        reminder_present: row.get("reminder_present"),
        ready_to_claim: row.get("ready"),
        issues: row.get("issues"),
    }
}
impl ScheduleOperationsStore for PostgresStore {
    fn audit_schedules(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ScheduleAudit>> {
        let owner = uuid(owner.as_str());
        let after = after.map(uuid).transpose();
        Box::pin(async move {
            let (owner, after) = (owner?, after?);
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            sqlx::query("SET LOCAL statement_timeout = '30s'")
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            sqlx::query("SELECT id FROM users WHERE id=$1")
                .bind(owner)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_error)?;
            let time: i64 = sqlx::query_scalar(
                "SELECT floor(extract(epoch FROM transaction_timestamp())*1000)::bigint",
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
            let summary = sqlx::query(&format!(
                "{AUDIT} SELECT count(*) AS tasks,
                count(*) FILTER (WHERE status='draft') AS drafts,
                count(*) FILTER (WHERE status='expired') AS expired_drafts,
                count(*) FILTER (WHERE status='scheduled') AS scheduled,
                count(*) FILTER (WHERE status='scheduled' AND ready) AS due,
                count(*) FILTER (WHERE status='running') AS running,
                count(*) FILTER (WHERE status='running' AND lease_until_ms<=$2) AS expired_leases,
                count(*) FILTER (WHERE status='delivered') AS delivered,
                count(*) FILTER (WHERE status='cancelled') AS cancelled,
                count(*) FILTER (WHERE status='failed') AS failed,
                count(*) FILTER (WHERE reminder_present) AS reminders,
                count(*) FILTER (WHERE cardinality(issues)>0) AS inconsistent,
                count(*) FILTER (WHERE ready) AS ready_count,
                min(run_at_ms) FILTER (WHERE ready) AS oldest,
                ($2::numeric-min(run_at_ms) FILTER (WHERE ready)::numeric)::text AS delay
                FROM audit"
            ))
            .bind(owner)
            .bind(time)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
            let rows=sqlx::query(&format!("{AUDIT} SELECT * FROM audit WHERE ($3::uuid IS NULL OR request_id>$3) ORDER BY request_id LIMIT 101"))
                .bind(owner).bind(time).bind(after).fetch_all(&mut *tx).await.map_err(map_error)?;
            let items: Vec<_> = rows.iter().take(100).map(item).collect();
            let next_cursor = (rows.len() > 100)
                .then(|| items.last().expect("full audit page").request_id.clone());
            let counts = ScheduleAuditCounts {
                tasks: summary.get("tasks"),
                drafts: summary.get("drafts"),
                expired_drafts: summary.get("expired_drafts"),
                scheduled: summary.get("scheduled"),
                due: summary.get("due"),
                running: summary.get("running"),
                expired_leases: summary.get("expired_leases"),
                delivered: summary.get("delivered"),
                cancelled: summary.get("cancelled"),
                failed: summary.get("failed"),
                reminders: summary.get("reminders"),
                inconsistent: summary.get("inconsistent"),
            };
            tx.commit().await.map_err(map_error)?;
            Ok(ScheduleAudit {
                user_id: owner.to_string(),
                snapshot_at_unix_ms: time.to_string(),
                worker_liveness: "unknown",
                consistent: counts.inconsistent == 0,
                counts,
                ready_to_claim: summary.get("ready_count"),
                oldest_ready_run_at_unix_ms: summary
                    .get::<Option<i64>, _>("oldest")
                    .map(|v| v.to_string()),
                oldest_ready_delay_ms: summary.get("delay"),
                items,
                next_cursor,
            })
        })
    }
}
