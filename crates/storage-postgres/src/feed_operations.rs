use crate::{PostgresStore, map_error, schedules::uuid};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageResult,
    feed_operations::{FeedAudit, FeedAuditItem, FeedOperationsStore},
};
use sqlx::Row;
use std::collections::BTreeMap;
use uuid::Uuid;

// Explicit projections allow column-only readers without access to URLs, plans or claims.
const AUDIT: &str = r"
WITH audit AS (
 SELECT c.request_id,c.subscription_id,c.status,c.reason,c.created_ms,c.claimed_ms,c.deadline_ms,
 array_remove(ARRAY[
 CASE WHEN c.claimed_ms<c.created_ms OR c.finished_ms<COALESCE(c.claimed_ms,c.created_ms)
   THEN 'invalid_time_order' END,
 CASE WHEN c.created_ms>$2 OR c.claimed_ms>$2 OR c.finished_ms>$2 THEN 'timestamp_in_future' END,
 CASE WHEN (SELECT count(*) FROM feed_collection_audit a WHERE a.user_id=c.user_id AND a.request_id=c.request_id)
   <> CASE WHEN c.status='draft' THEN 1 WHEN c.status IN ('running','cancelled') THEN 2 ELSE 3 END
   OR NOT EXISTS (SELECT 1 FROM feed_collection_audit a WHERE a.user_id=c.user_id AND a.request_id=c.request_id
     AND a.event='draft' AND a.at_ms=c.created_ms AND a.reason IS NULL AND a.inserted=0 AND a.updated=0 AND a.unchanged=0)
   OR (c.claimed_ms IS NOT NULL AND NOT EXISTS (SELECT 1 FROM feed_collection_audit a WHERE a.user_id=c.user_id AND a.request_id=c.request_id
     AND a.event='running' AND a.at_ms=c.claimed_ms AND a.reason IS NULL AND a.inserted=0 AND a.updated=0 AND a.unchanged=0))
   OR (c.finished_ms IS NOT NULL AND NOT EXISTS (SELECT 1 FROM feed_collection_audit a WHERE a.user_id=c.user_id AND a.request_id=c.request_id
     AND a.event=c.status AND a.at_ms=c.finished_ms AND a.reason IS NOT DISTINCT FROM c.reason
     AND a.inserted=c.inserted AND a.updated=c.updated AND a.unchanged=c.unchanged))
   THEN 'audit_mismatch' END
 ],NULL) AS issues
 FROM feed_collections c WHERE c.user_id=$1
)
";
impl FeedOperationsStore for PostgresStore {
    fn audit_feeds(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedAudit>> {
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
                "SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint",
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
            let counts = counts(&mut tx, owner, time).await?;
            let rows = sqlx::query(&format!("{AUDIT} SELECT request_id,subscription_id,status,reason,created_ms,deadline_ms,issues FROM audit WHERE ($3::uuid IS NULL OR request_id>$3) ORDER BY request_id LIMIT 101"))
                .bind(owner).bind(time).bind(after).fetch_all(&mut *tx).await.map_err(map_error)?;
            let items: Vec<_> = rows
                .iter()
                .take(100)
                .map(|r| FeedAuditItem {
                    request_id: r.get::<Uuid, _>("request_id").to_string(),
                    subscription_id: r.get::<Uuid, _>("subscription_id").to_string(),
                    status: r.get("status"),
                    reason: r.get("reason"),
                    created_at_unix_ms: r.get::<i64, _>("created_ms").to_string(),
                    deadline_unix_ms: r
                        .get::<Option<i64>, _>("deadline_ms")
                        .map(|n| n.to_string()),
                    issues: r.get("issues"),
                })
                .collect();
            let next_cursor = (rows.len() > 100)
                .then(|| items.last().expect("full audit page").request_id.clone());
            let mut issues = Vec::new();
            for key in [
                "inconsistent_collections",
                "entries_on_deleted_subscriptions",
                "invalid_entry_times",
            ] {
                if counts[key] > 0 {
                    issues.push(key.to_owned());
                }
            }
            for (key, limit) in [
                ("subscriptions", 1000),
                ("enabled_subscriptions", 50),
                ("running", 1),
                ("previews_last_24h", 100),
                ("claimed_today", 20),
            ] {
                if counts[key] > limit {
                    issues.push(format!("{key}_limit_exceeded"));
                }
            }
            let warnings = ["expired_drafts", "overdue_running", "unknown"]
                .into_iter()
                .filter(|key| counts[*key] > 0)
                .map(str::to_owned)
                .collect();
            tx.commit().await.map_err(map_error)?;
            Ok(FeedAudit {
                user_id: owner.to_string(),
                snapshot_at_unix_ms: time.to_string(),
                remaining_previews: (100 - counts["previews_last_24h"]).max(0),
                remaining_collections_today: (20 - counts["claimed_today"]).max(0),
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
    owner: Uuid,
    time: i64,
) -> StorageResult<BTreeMap<String, i64>> {
    let row = sqlx::query(&format!("{AUDIT} SELECT count(*) AS collections,
    count(*) FILTER (WHERE status='draft') AS draft,
    count(*) FILTER (WHERE status='running') AS running,
    count(*) FILTER (WHERE status='succeeded') AS succeeded,
    count(*) FILTER (WHERE status='failed') AS failed,
    count(*) FILTER (WHERE status='unknown') AS unknown,
    count(*) FILTER (WHERE status='cancelled') AS cancelled,
    count(*) FILTER (WHERE status='draft' AND created_ms::numeric+300000<=$2) AS expired_drafts,
    count(*) FILTER (WHERE status='running' AND deadline_ms<=$2) AS overdue_running,
    count(*) FILTER (WHERE created_ms>=$2-86400000) AS previews_last_24h,
    count(*) FILTER (WHERE claimed_ms>=$2/86400000*86400000 AND claimed_ms<$2/86400000*86400000+86400000) AS claimed_today,
    count(*) FILTER (WHERE cardinality(issues)>0) AS inconsistent_collections FROM audit"))
    .bind(owner).bind(time).fetch_one(&mut *connection).await.map_err(map_error)?;
    let mut counts = BTreeMap::new();
    for key in [
        "collections",
        "draft",
        "running",
        "succeeded",
        "failed",
        "unknown",
        "cancelled",
        "expired_drafts",
        "overdue_running",
        "previews_last_24h",
        "claimed_today",
        "inconsistent_collections",
    ] {
        counts.insert(key.to_owned(), row.get::<i64, _>(key));
    }
    let row = sqlx::query("SELECT count(*) AS subscriptions,
    count(*) FILTER (WHERE enabled AND NOT deleted) AS enabled_subscriptions,
    count(*) FILTER (WHERE deleted) AS deleted_subscriptions FROM feed_subscriptions WHERE user_id=$1")
    .bind(owner).fetch_one(&mut *connection).await.map_err(map_error)?;
    for key in [
        "subscriptions",
        "enabled_subscriptions",
        "deleted_subscriptions",
    ] {
        counts.insert(key.to_owned(), row.get::<i64, _>(key));
    }
    let row = sqlx::query("SELECT count(*) AS entries,
    count(*) FILTER (WHERE s.deleted) AS entries_on_deleted_subscriptions,
    count(*) FILTER (WHERE e.first_seen_ms>e.updated_ms OR e.updated_ms>e.last_seen_ms OR e.last_seen_ms>$2 OR e.first_seen_ms<=0) AS invalid_entry_times
    FROM feed_entries e JOIN feed_subscriptions s ON s.user_id=e.user_id AND s.id=e.subscription_id WHERE e.user_id=$1")
    .bind(owner).bind(time).fetch_one(&mut *connection).await.map_err(map_error)?;
    for key in [
        "entries",
        "entries_on_deleted_subscriptions",
        "invalid_entry_times",
    ] {
        counts.insert(key.to_owned(), row.get::<i64, _>(key));
    }
    Ok(counts)
}
