use crate::{
    PostgresStore, map_error,
    schedules::{lock_owner, now},
};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    brief_schedules::{BriefSchedule, BriefScheduleStore},
};
use sqlx::{Acquire, PgConnection, Row, postgres::PgRow};
use uuid::Uuid;
const DAY: i64 = 86_400_000;
fn invalid() -> StorageError {
    StorageError::InvalidData("invalid brief schedule".into())
}
fn id(owner: &UserId) -> StorageResult<Uuid> {
    Uuid::parse_str(owner.as_str()).map_err(|_| invalid())
}
fn record(row: &PgRow) -> StorageResult<BriefSchedule> {
    Ok(BriefSchedule {
        revision: u64::try_from(row.get::<i64, _>("revision")).map_err(|_| invalid())?,
        enabled: row.get("enabled"),
        minute_utc: u16::try_from(row.get::<i32, _>("minute_utc")).map_err(|_| invalid())?,
        next_run_unix_ms: row.get("next_run_ms"),
        last_attempt_unix_ms: row.get("last_attempt_ms"),
        last_request_id: row
            .get::<Option<Uuid>, _>("last_request_id")
            .map(|id| id.to_string()),
        last_outcome: row.get("last_outcome"),
    })
}
async fn read(tx: &mut PgConnection, owner: Uuid) -> StorageResult<BriefSchedule> {
    sqlx::query("SELECT revision,enabled,minute_utc,next_run_ms,last_attempt_ms,last_request_id,last_outcome FROM feed_brief_schedules WHERE user_id=$1")
        .bind(owner).fetch_optional(tx).await.map_err(map_error)?.as_ref().map(record).transpose().map(Option::unwrap_or_default)
}
fn next(time: i64, minute: u16, attempted: Option<i64>) -> StorageResult<i64> {
    let start = time / DAY * DAY;
    let target = start
        .checked_add(i64::from(minute) * 60_000)
        .ok_or_else(invalid)?;
    if target <= time || attempted.is_some_and(|last| last / DAY >= time / DAY) {
        target.checked_add(DAY).ok_or_else(invalid)
    } else {
        Ok(target)
    }
}
impl BriefScheduleStore for PostgresStore {
    fn get_brief_schedule(&self, owner: &UserId) -> BoxFuture<'_, StorageResult<BriefSchedule>> {
        let owner = id(owner);
        Box::pin(
            async move { read(&mut *self.pool.acquire().await.map_err(map_error)?, owner?).await },
        )
    }
    fn save_brief_schedule(
        &self,
        owner: &UserId,
        revision: u64,
        enabled: bool,
        minute: u16,
    ) -> BoxFuture<'_, StorageResult<BriefSchedule>> {
        let owner = id(owner);
        Box::pin(async move {
            if minute >= 1440 || revision > i64::MAX as u64 {
                return Err(invalid());
            }
            let owner = owner?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            lock_owner(&mut tx, owner).await?;
            let current = read(&mut tx, owner).await?;
            let same = current.enabled == enabled && current.minute_utc == minute;
            if same
                && current.revision > 0
                && (current.revision == revision
                    || revision.checked_add(1) == Some(current.revision))
            {
                tx.commit().await.map_err(map_error)?;
                return Ok(current);
            }
            if current.revision != revision {
                return Err(StorageError::Conflict("brief schedule changed".into()));
            }
            let updated = i64::try_from(revision)
                .map_err(|_| invalid())?
                .checked_add(1)
                .ok_or_else(invalid)?;
            let time = now(&mut tx).await?;
            let next_run = if enabled {
                Some(next(time, minute, current.last_attempt_unix_ms)?)
            } else {
                None
            };
            sqlx::query("INSERT INTO feed_brief_schedules(user_id,revision,enabled,minute_utc,next_run_ms) VALUES($1,$2,$3,$4,$5) ON CONFLICT(user_id) DO UPDATE SET revision=EXCLUDED.revision,enabled=EXCLUDED.enabled,minute_utc=EXCLUDED.minute_utc,next_run_ms=EXCLUDED.next_run_ms")
                .bind(owner).bind(updated).bind(enabled).bind(i32::from(minute)).bind(next_run).execute(&mut *tx).await.map_err(map_error)?;
            let result = read(&mut tx, owner).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(result)
        })
    }
    fn generate_due_brief(&self) -> BoxFuture<'_, StorageResult<bool>> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            sqlx::query("SET LOCAL statement_timeout='5s'")
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            let owner: Option<Uuid> = sqlx::query_scalar("SELECT u.id FROM users u JOIN feed_brief_schedules s ON s.user_id=u.id WHERE s.enabled AND s.next_run_ms<=floor(extract(epoch FROM clock_timestamp())*1000)::bigint ORDER BY s.next_run_ms,u.id LIMIT 1 FOR UPDATE OF u SKIP LOCKED")
                .fetch_optional(&mut *tx).await.map_err(map_error)?;
            let Some(owner) = owner else {
                tx.commit().await.map_err(map_error)?;
                return Ok(false);
            };
            let config = read(&mut tx, owner).await?;
            let time = now(&mut tx).await?;
            if !config.enabled || config.next_run_unix_ms.is_none_or(|due| due > time) {
                tx.commit().await.map_err(map_error)?;
                return Ok(false);
            }
            let today_target = time / DAY * DAY + i64::from(config.minute_utc) * 60_000;
            if today_target > time
                || config
                    .last_attempt_unix_ms
                    .is_some_and(|last| last / DAY >= time / DAY)
            {
                sqlx::query("UPDATE feed_brief_schedules SET next_run_ms=$2 WHERE user_id=$1")
                    .bind(owner)
                    .bind(next(time, config.minute_utc, config.last_attempt_unix_ms)?)
                    .execute(&mut *tx)
                    .await
                    .map_err(map_error)?;
            } else {
                let pref = crate::briefs::preferences(&mut tx, owner).await?;
                let mut attempt = tx.begin().await.map_err(map_error)?;
                let result = crate::briefs::create_in_transaction(
                    &mut attempt,
                    owner,
                    Uuid::new_v4(),
                    pref.revision,
                    time,
                )
                .await;
                let (outcome, request) = match result {
                    Ok(saved) => {
                        attempt.commit().await.map_err(map_error)?;
                        (
                            "generated",
                            Some(Uuid::parse_str(&saved.request_id).map_err(|_| invalid())?),
                        )
                    }
                    Err(StorageError::Conflict(_) | StorageError::InvalidData(_)) => {
                        attempt.rollback().await.map_err(map_error)?;
                        ("skipped", None)
                    }
                    Err(error) => {
                        attempt.rollback().await.map_err(map_error)?;
                        return Err(error);
                    }
                };
                sqlx::query("UPDATE feed_brief_schedules SET next_run_ms=$2,last_attempt_ms=$3,last_request_id=$4,last_outcome=$5 WHERE user_id=$1")
                    .bind(owner).bind(next(time, config.minute_utc, Some(time))?).bind(time).bind(request).bind(outcome).execute(&mut *tx).await.map_err(map_error)?;
            }
            tx.commit().await.map_err(map_error)?;
            Ok(true)
        })
    }
}
