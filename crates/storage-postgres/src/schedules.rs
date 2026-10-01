use crate::{PostgresStore, map_error};
use personal_ai_agent_core::schedules::schedule_digest;
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    schedules::{
        NewSchedule, SCHEDULE_VERSION, Schedule, ScheduleApproval, SchedulePage, ScheduleStore,
    },
};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

pub(super) const FIELDS: &str = "request_id,version,title,body,run_at_ms,digest,CASE WHEN status='draft' AND approval_expires_ms<=floor(extract(epoch FROM clock_timestamp())*1000)::bigint THEN 'expired' ELSE status END AS effective_status,created_ms,approval_expires_ms,approved_ms,cancelled_ms,delivered_ms";
pub(super) fn uuid(value: &str) -> StorageResult<Uuid> {
    Uuid::parse_str(value).map_err(|_| StorageError::InvalidData("invalid schedule id".into()))
}
fn conflict() -> StorageError {
    StorageError::Conflict("schedule changed or authorization expired".into())
}
pub(super) fn record(row: &PgRow) -> Schedule {
    Schedule {
        request_id: row.get::<Uuid, _>("request_id").to_string(),
        version: row.get("version"),
        title: row.get("title"),
        body: row.get("body"),
        run_at_unix_ms: row.get("run_at_ms"),
        max_runs: 1,
        amount_micro: 0,
        digest: row.get("digest"),
        status: row.get("effective_status"),
        created_at_unix_ms: row.get("created_ms"),
        approval_expires_at_unix_ms: row.get("approval_expires_ms"),
        approved_at_unix_ms: row.get("approved_ms"),
        cancelled_at_unix_ms: row.get("cancelled_ms"),
        delivered_at_unix_ms: row.get("delivered_ms"),
    }
}
pub(super) async fn lock_owner(tx: &mut sqlx::PgConnection, owner: Uuid) -> StorageResult<()> {
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(owner)
        .fetch_one(tx)
        .await
        .map_err(map_error)?;
    Ok(())
}
pub(super) async fn now(tx: &mut sqlx::PgConnection) -> StorageResult<i64> {
    sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
        .fetch_one(tx)
        .await
        .map_err(map_error)
}
async fn read(tx: &mut sqlx::PgConnection, owner: Uuid, request: Uuid) -> StorageResult<Schedule> {
    let row = sqlx::query(&format!(
        "SELECT {FIELDS} FROM schedules WHERE user_id=$1 AND request_id=$2"
    ))
    .bind(owner)
    .bind(request)
    .fetch_one(tx)
    .await
    .map_err(map_error)?;
    Ok(record(&row))
}

impl ScheduleStore for PostgresStore {
    fn create_schedule(
        &self,
        owner: &UserId,
        input: &NewSchedule,
    ) -> BoxFuture<'_, StorageResult<Schedule>> {
        let owner = uuid(owner.as_str());
        let mut input = input.clone();
        Box::pin(async move {
            input.validate()?;
            let (owner, request) = (owner?, uuid(&input.request_id)?);
            input.request_id = request.to_string();
            let digest = schedule_digest(&UserId::new(owner.to_string()), &input);
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            lock_owner(&mut tx, owner).await?;
            let existing = sqlx::query(&format!(
                "SELECT {FIELDS} FROM schedules WHERE user_id=$1 AND request_id=$2"
            ))
            .bind(owner)
            .bind(request)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_error)?;
            if let Some(row) = existing {
                let saved = record(&row);
                if saved.digest != digest {
                    return Err(conflict());
                }
                tx.commit().await.map_err(map_error)?;
                return Ok(saved);
            }
            let now = now(&mut tx).await?;
            if input.run_at_unix_ms < now + 60_000 || input.run_at_unix_ms > now + 365 * 86_400_000
            {
                return Err(StorageError::InvalidData(
                    "schedule must be 1 minute to 365 days ahead".into(),
                ));
            }
            let counts=sqlx::query("SELECT count(*) AS total,count(*) FILTER (WHERE created_ms>$2-86400000) AS daily FROM schedules WHERE user_id=$1")
                .bind(owner).bind(now).fetch_one(&mut *tx).await.map_err(map_error)?;
            if counts.get::<i64, _>("total") >= 1000 || counts.get::<i64, _>("daily") >= 100 {
                return Err(StorageError::Conflict("schedule quota reached".into()));
            }
            sqlx::query("INSERT INTO schedules(user_id,request_id,version,title,body,run_at_ms,digest,created_ms,approval_expires_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
                .bind(owner).bind(request).bind(SCHEDULE_VERSION).bind(&input.title).bind(&input.body).bind(input.run_at_unix_ms).bind(digest).bind(now).bind((now+900_000).min(input.run_at_unix_ms)).execute(&mut *tx).await.map_err(map_error)?;
            let saved = read(&mut tx, owner, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn get_schedule(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Schedule>> {
        let ids = uuid(owner.as_str()).and_then(|owner| Ok((owner, uuid(request)?)));
        Box::pin(async move {
            let (owner, request) = ids?;
            let mut connection = self.pool.acquire().await.map_err(map_error)?;
            read(&mut connection, owner, request).await
        })
    }
    fn list_schedules(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<SchedulePage>> {
        let owner = uuid(owner.as_str());
        let after = after.map(uuid).transpose();
        Box::pin(async move {
            let rows=sqlx::query(&format!("SELECT {FIELDS} FROM schedules WHERE user_id=$1 AND ($2::uuid IS NULL OR request_id>$2) ORDER BY request_id LIMIT 21"))
                .bind(owner?).bind(after?).fetch_all(&self.pool).await.map_err(map_error)?;
            let items: Vec<_> = rows.iter().take(20).map(record).collect();
            let next_cursor = (rows.len() > 20)
                .then(|| items.last().expect("full schedule page").request_id.clone());
            Ok(SchedulePage { items, next_cursor })
        })
    }
    fn approve_schedule(
        &self,
        owner: &UserId,
        request: &str,
        approval: &ScheduleApproval,
    ) -> BoxFuture<'_, StorageResult<Schedule>> {
        let ids = uuid(owner.as_str()).and_then(|owner| Ok((owner, uuid(request)?)));
        let approval = approval.clone();
        Box::pin(async move {
            let (owner, request) = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            lock_owner(&mut tx, owner).await?;
            let saved = read(&mut tx, owner, request).await?;
            let input = NewSchedule {
                request_id: saved.request_id.clone(),
                title: saved.title.clone(),
                body: saved.body.clone(),
                run_at_unix_ms: saved.run_at_unix_ms,
            };
            if saved.version != SCHEDULE_VERSION
                || saved.digest != schedule_digest(&UserId::new(owner.to_string()), &input)
                || approval.digest != saved.digest
                || approval.accepted_run_at_unix_ms != saved.run_at_unix_ms
                || approval.accepted_max_runs != 1
                || approval.accepted_amount_micro != 0
                || !approval.acknowledge_schedule
            {
                return Err(conflict());
            }
            if matches!(saved.status.as_str(), "scheduled" | "running" | "delivered") {
                tx.commit().await.map_err(map_error)?;
                return Ok(saved);
            }
            let now = now(&mut tx).await?;
            if saved.status != "draft" || now >= saved.approval_expires_at_unix_ms {
                return Err(conflict());
            }
            let approval = serde_json::to_value(approval).map_err(|_| conflict())?;
            sqlx::query("UPDATE schedules SET status='scheduled',approved_ms=$3,approval=$4 WHERE user_id=$1 AND request_id=$2")
                .bind(owner).bind(request).bind(now).bind(approval).execute(&mut *tx).await.map_err(map_error)?;
            let saved = read(&mut tx, owner, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn cancel_schedule(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Schedule>> {
        let ids = uuid(owner.as_str()).and_then(|owner| Ok((owner, uuid(request)?)));
        Box::pin(async move {
            let (owner, request) = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            lock_owner(&mut tx, owner).await?;
            let saved = read(&mut tx, owner, request).await?;
            if saved.status == "delivered" {
                tx.commit().await.map_err(map_error)?;
                return Ok(saved);
            }
            sqlx::query("UPDATE schedules SET status='cancelled',claim_id=NULL,lease_until_ms=NULL,cancelled_ms=COALESCE(cancelled_ms,floor(extract(epoch FROM clock_timestamp())*1000)::bigint) WHERE user_id=$1 AND request_id=$2")
                .bind(owner).bind(request).execute(&mut *tx).await.map_err(map_error)?;
            let saved = read(&mut tx, owner, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
}
