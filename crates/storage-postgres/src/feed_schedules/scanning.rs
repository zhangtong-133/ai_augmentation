use crate::{PostgresStore, feeds::id, map_error};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageResult,
    feed_schedules::{FeedScheduleCursor, FeedScheduleScan, FeedScheduleScanStore},
};
use sqlx::Row;
use uuid::Uuid;

const DUE: &str = "WITH clock AS (SELECT floor(extract(epoch FROM statement_timestamp())*1000)::bigint AS now_ms), candidates AS (
 SELECT s.user_id,s.id,(s.plan#>>'{input,starts_at_unix_ms}')::bigint AS start_ms,
 (s.plan#>>'{input,interval_hours}')::bigint*3600000 AS interval_ms,c.now_ms
 FROM feed_schedules s CROSS JOIN clock c
 WHERE s.status='active' AND s.ends_ms>c.now_ms AND s.approved_ms IS NOT NULL
 AND ($1::uuid IS NULL OR (s.user_id,s.id)>($1,$2))
 AND (s.plan#>>'{input,interval_hours}') IN ('1','6','24')
), slots AS (
 SELECT *, start_ms+((now_ms-start_ms)/interval_ms)*interval_ms AS slot_ms
 FROM candidates WHERE start_ms<=now_ms
)
SELECT user_id,id FROM slots s WHERE now_ms<slot_ms+600000
AND NOT EXISTS (SELECT 1 FROM feed_schedule_occurrences o WHERE o.user_id=s.user_id AND o.schedule_id=s.id AND o.scheduled_ms=s.slot_ms)
ORDER BY user_id,id LIMIT 21";
const EXPIRED: &str = "SELECT c.user_id,c.request_id AS id FROM feed_collections c
JOIN feed_schedule_occurrences o ON o.user_id=c.user_id AND o.request_id=c.request_id
WHERE c.status='running' AND c.deadline_ms<=floor(extract(epoch FROM statement_timestamp())*1000)::bigint
AND ($1::uuid IS NULL OR (c.user_id,c.request_id)>($1,$2)) ORDER BY c.user_id,c.request_id LIMIT 21";

impl FeedScheduleScanStore for PostgresStore {
    fn scan_due_feed_schedules(
        &self,
        after: Option<&FeedScheduleCursor>,
    ) -> BoxFuture<'_, StorageResult<FeedScheduleScan>> {
        scan(self, DUE, after)
    }
    fn scan_expired_scheduled_collections(
        &self,
        after: Option<&FeedScheduleCursor>,
    ) -> BoxFuture<'_, StorageResult<FeedScheduleScan>> {
        scan(self, EXPIRED, after)
    }
}
fn scan<'a>(
    store: &'a PostgresStore,
    sql: &'static str,
    after: Option<&FeedScheduleCursor>,
) -> BoxFuture<'a, StorageResult<FeedScheduleScan>> {
    let cursor = after
        .map(|c| Ok((id(c.owner.as_str())?, id(&c.id)?)))
        .transpose();
    Box::pin(async move {
        let cursor: Option<(Uuid, Uuid)> = cursor?;
        let rows = sqlx::query(sql)
            .bind(cursor.map(|c| c.0))
            .bind(cursor.map(|c| c.1))
            .fetch_all(&store.pool)
            .await
            .map_err(map_error)?;
        let mut items: Vec<_> = rows
            .iter()
            .map(|r| FeedScheduleCursor {
                owner: UserId::new(r.get::<Uuid, _>("user_id").to_string()),
                id: r.get::<Uuid, _>("id").to_string(),
            })
            .collect();
        let more = items.len() > 20;
        items.truncate(20);
        let next_cursor = if more { items.last().cloned() } else { None };
        Ok(FeedScheduleScan { items, next_cursor })
    })
}
