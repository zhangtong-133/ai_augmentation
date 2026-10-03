use super::read;
use crate::{
    PostgresStore,
    feeds::{audit, check_collection_quota, conflict, id, invalid, locked, now, read_sub},
    map_error,
};
use personal_ai_domain::UserId;
use personal_ai_feeds::{plan_collection, schedule::authorize_schedule};
use personal_ai_storage::{
    BoxFuture, StorageResult,
    feed_schedules::{FeedScheduleExecutionStore, FeedScheduleStatus},
    feeds::CollectionClaim,
};
use sqlx::Row;
use uuid::Uuid;

impl FeedScheduleExecutionStore for PostgresStore {
    fn claim_scheduled_collection(
        &self,
        owner: &UserId,
        schedule: &str,
    ) -> BoxFuture<'_, StorageResult<CollectionClaim>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, id(schedule)?)));
        Box::pin(async move {
            let (owner, schedule) = ids?;
            let mut tx = locked(self, owner).await?;
            let saved = read(&mut tx, owner, schedule).await?;
            if saved.status != FeedScheduleStatus::Active {
                return Err(conflict());
            }
            let source = read_sub(&mut tx, owner, id(&saved.plan.subscription_id)?).await?;
            if source.deleted {
                return Err(conflict());
            }
            let user = UserId::new(owner.to_string());
            let authorized = authorize_schedule(
                &user,
                &source.snapshot,
                &saved.plan,
                &saved.digest,
                u64::try_from(saved.approved_at_unix_ms.ok_or_else(conflict)?)
                    .map_err(|_| invalid())?,
            )
            .map_err(|_| conflict())?;
            let time = now(&mut tx).await?;
            let due = authorized
                .due(
                    &user,
                    &source.snapshot,
                    true,
                    u64::try_from(time).map_err(|_| invalid())?,
                )
                .map_err(|_| conflict())?
                .ok_or_else(conflict)?;
            let already_claimed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM feed_schedule_occurrences WHERE user_id=$1 AND schedule_id=$2 AND occurrence_key=$3)").bind(owner).bind(schedule).bind(&due.occurrence_key).fetch_one(&mut *tx).await.map_err(map_error)?;
            if already_claimed {
                return Err(conflict());
            }
            check_collection_quota(&mut tx, owner, time).await?;
            let request = Uuid::new_v4();
            let claim = Uuid::new_v4();
            let plan = plan_collection(
                &user,
                &source.snapshot,
                &request.to_string(),
                u64::try_from(time).map_err(|_| invalid())?,
            )
            .map_err(|_| conflict())?;
            let digest = plan.consent_digest().map_err(|_| invalid())?;
            sqlx::query("INSERT INTO feed_collections(user_id,request_id,subscription_id,plan,digest,status,created_ms) VALUES($1,$2,$3,$4,$5,'draft',$6)")
                .bind(owner).bind(request).bind(id(&plan.subscription_id)?).bind(serde_json::to_value(&plan).map_err(|_| invalid())?).bind(&digest).bind(time).execute(&mut *tx).await.map_err(map_error)?;
            audit(&mut tx, owner, request).await?;
            sqlx::query("UPDATE feed_collections SET status='running',accepted_digest=$3,claimed_ms=$4,deadline_ms=$4+60000,claim_id=$5 WHERE user_id=$1 AND request_id=$2")
                .bind(owner).bind(request).bind(digest).bind(time).bind(claim).execute(&mut *tx).await.map_err(map_error)?;
            sqlx::query("INSERT INTO feed_schedule_occurrences(user_id,schedule_id,occurrence_key,request_id,scheduled_ms,dispatch_expires_ms) VALUES($1,$2,$3,$4,$5,$6)")
                .bind(owner).bind(schedule).bind(due.occurrence_key).bind(request)
                .bind(i64::try_from(due.scheduled_at_unix_ms).map_err(|_| invalid())?).bind(i64::try_from(due.dispatch_expires_at_unix_ms).map_err(|_| invalid())?).execute(&mut *tx).await.map_err(map_error)?;
            audit(&mut tx, owner, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(CollectionClaim {
                owner: user,
                request_id: request.to_string(),
                claim_id: claim.to_string(),
                plan,
            })
        })
    }
    fn dispatch_scheduled_collection(
        &self,
        claim: &CollectionClaim,
    ) -> BoxFuture<'_, StorageResult<bool>> {
        let claim = claim.clone();
        Box::pin(async move {
            let owner = id(claim.owner.as_str())?;
            let request = id(&claim.request_id)?;
            let mut tx = locked(self, owner).await?;
            let row=sqlx::query("SELECT o.schedule_id,o.scheduled_ms,o.dispatch_expires_ms,o.dispatched_ms,c.claim_id,c.plan,c.status,c.deadline_ms FROM feed_schedule_occurrences o JOIN feed_collections c ON c.user_id=o.user_id AND c.request_id=o.request_id WHERE o.user_id=$1 AND o.request_id=$2")
                .bind(owner).bind(request).fetch_one(&mut *tx).await.map_err(map_error)?;
            let plan: personal_ai_feeds::CollectionPlan =
                serde_json::from_value(row.get("plan")).map_err(|_| invalid())?;
            if row.get::<Uuid, _>("claim_id") != id(&claim.claim_id)? || plan != claim.plan {
                return Err(conflict());
            }
            if row.get::<Option<i64>, _>("dispatched_ms").is_some()
                || row.get::<String, _>("status") != "running"
            {
                return Ok(false);
            }
            let schedule = read(&mut tx, owner, row.get("schedule_id")).await?;
            let source = read_sub(&mut tx, owner, id(&plan.subscription_id)?).await?;
            let time = now(&mut tx).await?;
            if time < row.get::<i64, _>("scheduled_ms")
                || schedule.status != FeedScheduleStatus::Active
                || source.deleted
                || time >= row.get::<i64, _>("dispatch_expires_ms")
                || time >= row.get::<i64, _>("deadline_ms")
            {
                return Ok(false);
            }
            let authorized = authorize_schedule(
                &claim.owner,
                &source.snapshot,
                &schedule.plan,
                &schedule.digest,
                u64::try_from(schedule.approved_at_unix_ms.ok_or_else(conflict)?)
                    .map_err(|_| invalid())?,
            )
            .map_err(|_| conflict())?;
            if authorized
                .due(
                    &claim.owner,
                    &source.snapshot,
                    true,
                    u64::try_from(time).map_err(|_| invalid())?,
                )
                .map_err(|_| conflict())?
                .is_none()
            {
                return Ok(false);
            }
            sqlx::query("UPDATE feed_schedule_occurrences SET dispatched_ms=$3 WHERE user_id=$1 AND request_id=$2").bind(owner).bind(request).bind(time).execute(&mut *tx).await.map_err(map_error)?;
            tx.commit().await.map_err(map_error)?;
            Ok(true)
        })
    }
}
