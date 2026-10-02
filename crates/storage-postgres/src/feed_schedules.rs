use crate::{
    PostgresStore,
    feeds::{conflict, id, invalid, locked, now, read_sub},
    map_error,
};
use personal_ai_domain::UserId;
use personal_ai_feeds::schedule::{ScheduleInput, SchedulePlan, authorize_schedule, plan_schedule};
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    feed_schedules::{FeedSchedule, FeedScheduleAudit, FeedScheduleStatus, FeedScheduleStore},
    feeds::FeedPage,
};
use sqlx::{PgConnection, Row, postgres::PgRow};
use uuid::Uuid;

fn decode(row: &PgRow) -> StorageResult<FeedSchedule> {
    let plan: SchedulePlan = serde_json::from_value(row.get("plan")).map_err(|_| invalid())?;
    let digest: String = row.get("digest");
    if plan.user_id != row.get::<Uuid, _>("user_id").to_string()
        || plan.input.schedule_id != row.get::<Uuid, _>("id").to_string()
        || plan.subscription_id != row.get::<Uuid, _>("subscription_id").to_string()
        || i64::try_from(plan.created_at_unix_ms).map_err(|_| invalid())?
            != row.get::<i64, _>("created_ms")
        || i64::try_from(plan.approval_expires_at_unix_ms).map_err(|_| invalid())?
            != row.get::<i64, _>("approval_expires_ms")
        || i64::try_from(plan.input.ends_at_unix_ms).map_err(|_| invalid())?
            != row.get::<i64, _>("ends_ms")
        || plan.consent_digest().map_err(|_| invalid())? != digest
    {
        return Err(conflict());
    }
    Ok(FeedSchedule {
        plan,
        digest,
        status: serde_json::from_value(serde_json::Value::String(row.get("status")))
            .map_err(|_| invalid())?,
        approved_at_unix_ms: row.get("approved_ms"),
    })
}
async fn expire(tx: &mut PgConnection, owner: Uuid, time: i64) -> StorageResult<()> {
    sqlx::query("UPDATE feed_schedules SET status='expired' WHERE user_id=$1 AND ((status='draft' AND approval_expires_ms<=$2) OR (status='active' AND ends_ms<=$2))")
        .bind(owner).bind(time).execute(tx).await.map_err(map_error)?;
    Ok(())
}
async fn read(tx: &mut PgConnection, owner: Uuid, schedule: Uuid) -> StorageResult<FeedSchedule> {
    let row = sqlx::query("SELECT * FROM feed_schedules WHERE user_id=$1 AND id=$2")
        .bind(owner)
        .bind(schedule)
        .fetch_one(tx)
        .await
        .map_err(map_error)?;
    decode(&row)
}
impl FeedScheduleStore for PostgresStore {
    fn preview_feed_schedule(
        &self,
        owner: &UserId,
        subscription: &str,
        input: &ScheduleInput,
    ) -> BoxFuture<'_, StorageResult<FeedSchedule>> {
        let ids =
            id(owner.as_str()).and_then(|o| Ok((o, id(subscription)?, id(&input.schedule_id)?)));
        let input = input.clone();
        Box::pin(async move {
            let (owner, sub, schedule) = ids?;
            let mut input = input;
            input.schedule_id = schedule.to_string();
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            if let Some(row) =
                sqlx::query("SELECT * FROM feed_schedules WHERE user_id=$1 AND id=$2")
                    .bind(owner)
                    .bind(schedule)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(map_error)?
            {
                let saved = decode(&row)?;
                if saved.plan.subscription_id != sub.to_string() || saved.plan.input != input {
                    return Err(conflict());
                }
                tx.commit().await.map_err(map_error)?;
                return Ok(saved);
            }
            let source = read_sub(&mut tx, owner, sub).await?;
            if source.deleted {
                return Err(StorageError::NotFound);
            }
            let plan = plan_schedule(
                &UserId::new(owner.to_string()),
                &source.snapshot,
                &input,
                u64::try_from(time).map_err(|_| invalid())?,
            )
            .map_err(|_| invalid())?;
            let total: i64 =
                sqlx::query_scalar("SELECT count(*) FROM feed_schedules WHERE user_id=$1")
                    .bind(owner)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(map_error)?;
            let daily: i64 = sqlx::query_scalar("SELECT count(*) FROM feed_schedules WHERE user_id=$1 AND created_ms >= $2 AND created_ms < $2+86400000").bind(owner).bind(time/86_400_000*86_400_000).fetch_one(&mut *tx).await.map_err(map_error)?;
            if total >= 1000 || daily >= 20 {
                return Err(conflict());
            }
            sqlx::query("INSERT INTO feed_schedules(user_id,id,subscription_id,plan,digest,status,created_ms,approval_expires_ms,ends_ms) VALUES($1,$2,$3,$4,$5,'draft',$6,$7,$8)")
                .bind(owner).bind(schedule).bind(sub).bind(serde_json::to_value(&plan).map_err(|_| invalid())?).bind(plan.consent_digest().map_err(|_| invalid())?).bind(time)
                .bind(i64::try_from(plan.approval_expires_at_unix_ms).map_err(|_| invalid())?).bind(i64::try_from(plan.input.ends_at_unix_ms).map_err(|_| invalid())?).execute(&mut *tx).await.map_err(map_error)?;
            let saved = read(&mut tx, owner, schedule).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn approve_feed_schedule(
        &self,
        owner: &UserId,
        schedule: &str,
        digest: &str,
    ) -> BoxFuture<'_, StorageResult<FeedSchedule>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, id(schedule)?)));
        let digest = digest.to_owned();
        Box::pin(async move {
            let (owner, schedule) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            let saved = read(&mut tx, owner, schedule).await?;
            if saved.digest != digest {
                return Err(conflict());
            }
            if saved.status == FeedScheduleStatus::Active {
                tx.commit().await.map_err(map_error)?;
                return Ok(saved);
            }
            if saved.status != FeedScheduleStatus::Draft {
                return Err(conflict());
            }
            let source = read_sub(&mut tx, owner, id(&saved.plan.subscription_id)?).await?;
            if source.deleted {
                return Err(conflict());
            }
            authorize_schedule(
                &UserId::new(owner.to_string()),
                &source.snapshot,
                &saved.plan,
                &digest,
                u64::try_from(time).map_err(|_| invalid())?,
            )
            .map_err(|_| conflict())?;
            let active: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM feed_schedules WHERE user_id=$1 AND subscription_id=$2 AND status='active')").bind(owner).bind(id(&saved.plan.subscription_id)?).fetch_one(&mut *tx).await.map_err(map_error)?;
            if active {
                return Err(conflict());
            }
            sqlx::query("UPDATE feed_schedules SET status='active',approved_ms=$3 WHERE user_id=$1 AND id=$2").bind(owner).bind(schedule).bind(time).execute(&mut *tx).await.map_err(map_error)?;
            let saved = read(&mut tx, owner, schedule).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn cancel_feed_schedule(
        &self,
        owner: &UserId,
        schedule: &str,
    ) -> BoxFuture<'_, StorageResult<FeedSchedule>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, id(schedule)?)));
        Box::pin(async move {
            let (owner, schedule) = ids?;
            let mut tx = locked(self, owner).await?;
            sqlx::query("UPDATE feed_schedules SET status='cancelled' WHERE user_id=$1 AND id=$2 AND status IN ('draft','active')").bind(owner).bind(schedule).execute(&mut *tx).await.map_err(map_error)?;
            let saved = read(&mut tx, owner, schedule).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn get_feed_schedule(
        &self,
        owner: &UserId,
        schedule: &str,
    ) -> BoxFuture<'_, StorageResult<FeedSchedule>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, id(schedule)?)));
        Box::pin(async move {
            let (owner, schedule) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            let saved = read(&mut tx, owner, schedule).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn list_feed_schedules(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedPage<FeedSchedule>>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, after.map(id).transpose()?)));
        Box::pin(async move {
            let (owner, after) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            let rows=sqlx::query("SELECT * FROM feed_schedules WHERE user_id=$1 AND ($2::uuid IS NULL OR id>$2) ORDER BY id LIMIT 21").bind(owner).bind(after).fetch_all(&mut *tx).await.map_err(map_error)?;
            let mut items = rows.iter().map(decode).collect::<StorageResult<Vec<_>>>()?;
            let more = items.len() > 20;
            items.truncate(20);
            let next_cursor = if more {
                items.last().map(|v| v.plan.input.schedule_id.clone())
            } else {
                None
            };
            tx.commit().await.map_err(map_error)?;
            Ok(FeedPage { items, next_cursor })
        })
    }
    fn feed_schedule_audit(
        &self,
        owner: &UserId,
        schedule: &str,
    ) -> BoxFuture<'_, StorageResult<Vec<FeedScheduleAudit>>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, id(schedule)?)));
        Box::pin(async move {
            let (owner, schedule) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            read(&mut tx, owner, schedule).await?;
            let rows=sqlx::query("SELECT event,at_ms FROM feed_schedule_audit WHERE user_id=$1 AND schedule_id=$2 ORDER BY at_ms,event").bind(owner).bind(schedule).fetch_all(&mut *tx).await.map_err(map_error)?;
            let events = rows
                .iter()
                .map(|r| FeedScheduleAudit {
                    event: r.get("event"),
                    at_unix_ms: r.get("at_ms"),
                })
                .collect();
            tx.commit().await.map_err(map_error)?;
            Ok(events)
        })
    }
}
