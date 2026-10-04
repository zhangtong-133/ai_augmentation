use crate::{PostgresStore, map_error, schedules::uuid};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageResult,
    feed_value_operations::{FeedValueAudit, FeedValueAuditItem, FeedValueOperationsStore},
};
use sqlx::Row;
use std::collections::BTreeMap;
const AUDIT: &str = include_str!("feed_value_operations.sql");
impl FeedValueOperationsStore for PostgresStore {
    fn audit_feed_values(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedValueAudit>> {
        let owner = uuid(owner.as_str());
        let after = after.map(uuid).transpose();
        Box::pin(async move {
            let (owner, after) = (owner?, after?);
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            sqlx::query("SET LOCAL statement_timeout = '10s'")
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            sqlx::query("SELECT id FROM users WHERE id=$1")
                .bind(owner)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_error)?;
            let time: i64 = sqlx::query_scalar(
                "SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint",
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
            let counts = counts(&mut tx, owner, time).await?;
            let rows = sqlx::query(&format!("{AUDIT} SELECT id,status,created_ms,expires_ms,approved_ms,dispatch_deadline_ms,sent_ms,issues FROM audit WHERE ($3::uuid IS NULL OR id>$3) ORDER BY id LIMIT 101"))
                .bind(owner).bind(time).bind(after).fetch_all(&mut *tx).await.map_err(map_error)?;
            let items: Vec<_> = rows
                .iter()
                .take(100)
                .map(|row| FeedValueAuditItem {
                    request_id: row.get::<uuid::Uuid, _>("id").to_string(),
                    status: row.get("status"),
                    created_at_unix_ms: row.get::<i64, _>("created_ms").to_string(),
                    expires_at_unix_ms: row.get::<i64, _>("expires_ms").to_string(),
                    approved_at_unix_ms: row
                        .get::<Option<i64>, _>("approved_ms")
                        .map(|v| v.to_string()),
                    dispatch_deadline_unix_ms: row
                        .get::<Option<i64>, _>("dispatch_deadline_ms")
                        .map(|v| v.to_string()),
                    sent_at_unix_ms: row.get::<Option<i64>, _>("sent_ms").map(|v| v.to_string()),
                    issues: row.get("issues"),
                })
                .collect();
            let next_cursor = (rows.len() > 100)
                .then(|| items.last().expect("full audit page").request_id.clone());
            let mut issues = Vec::new();
            if counts["inconsistent_records"] > 0 {
                issues.push("inconsistent_records".into());
            }
            for (key, limit) in [("records", 1000), ("previews_today", 20)] {
                if counts[key] > limit {
                    issues.push(format!("{key}_limit_exceeded"));
                }
            }
            let warnings = ["expired_active", "overdue_running", "unknown"]
                .into_iter()
                .filter(|key| counts[*key] > 0)
                .map(str::to_owned)
                .collect();
            tx.commit().await.map_err(map_error)?;
            Ok(FeedValueAudit {
                user_id: owner.to_string(),
                snapshot_at_unix_ms: time.to_string(),
                remaining_previews_today: (20 - counts["previews_today"]).max(0),
                remaining_records: (1000 - counts["records"]).max(0),
                consistent: issues.is_empty(),
                counts,
                issues,
                warnings,
                items,
                next_cursor,
            })
        })
    }
}

async fn counts(
    connection: &mut sqlx::PgConnection,
    owner: uuid::Uuid,
    time: i64,
) -> StorageResult<BTreeMap<String, i64>> {
    let totals = sqlx::query(&format!("{AUDIT} SELECT count(*) AS records,
                count(*) FILTER(WHERE status='draft') AS draft,
                count(*) FILTER(WHERE status='authorized') AS authorized,
                count(*) FILTER(WHERE status='running') AS running,
                count(*) FILTER(WHERE status='succeeded') AS succeeded,
                count(*) FILTER(WHERE status='unknown') AS unknown,
                count(*) FILTER(WHERE status='cancelled') AS cancelled,
                count(*) FILTER(WHERE status='expired') AS expired,
                count(*) FILTER(WHERE status='invalidated') AS invalidated,
                count(*) FILTER(WHERE sent_ms IS NOT NULL) AS sent,
                count(*) FILTER(WHERE created_ms>=$2/86400000*86400000 AND created_ms<$2/86400000*86400000+86400000) AS previews_today,
                count(*) FILTER(WHERE status IN ('draft','authorized') AND expires_ms<=$2) AS expired_active,
                count(*) FILTER(WHERE status='running' AND dispatch_deadline_ms<=$2) AS overdue_running,
                count(*) FILTER(WHERE cardinality(issues)>0) AS inconsistent_records FROM audit"))
                .bind(owner).bind(time).fetch_one(&mut *connection).await.map_err(map_error)?;
    let mut counts = BTreeMap::new();
    for key in [
        "records",
        "draft",
        "authorized",
        "running",
        "succeeded",
        "unknown",
        "cancelled",
        "expired",
        "invalidated",
        "sent",
        "previews_today",
        "expired_active",
        "overdue_running",
        "inconsistent_records",
    ] {
        counts.insert(key.to_owned(), totals.get::<i64, _>(key));
    }
    Ok(counts)
}
