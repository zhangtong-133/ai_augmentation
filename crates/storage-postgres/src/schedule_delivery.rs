use crate::{
    PostgresStore, map_error,
    schedules::{FIELDS, lock_owner, now, record, uuid},
};
use personal_ai_agent_core::schedules::schedule_digest;
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    schedules::{
        NewSchedule, ReminderPage, SCHEDULE_VERSION, ScheduleApproval, ScheduleDeliveryStore,
        ScheduleLease, ScheduleReminder,
    },
};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

fn conflict() -> StorageError {
    StorageError::Conflict("schedule lease or authorization changed".into())
}
fn authorized(owner: Uuid, row: &PgRow) -> bool {
    let saved = record(row);
    let input = NewSchedule {
        request_id: saved.request_id,
        title: saved.title,
        body: saved.body,
        run_at_unix_ms: saved.run_at_unix_ms,
    };
    let approval = row
        .get::<Option<serde_json::Value>, _>("approval")
        .and_then(|v| serde_json::from_value::<ScheduleApproval>(v).ok());
    input.validate().is_ok()
        && saved.version == SCHEDULE_VERSION
        && saved.digest == schedule_digest(&UserId::new(owner.to_string()), &input)
        && saved.approved_at_unix_ms.is_some_and(|time| {
            time >= saved.created_at_unix_ms && time < saved.approval_expires_at_unix_ms
        })
        && approval.is_some_and(|a| {
            a.digest == saved.digest
                && a.accepted_run_at_unix_ms == input.run_at_unix_ms
                && a.accepted_max_runs == 1
                && a.accepted_amount_micro == 0
                && a.acknowledge_schedule
        })
}
fn reminder(row: &PgRow) -> ScheduleReminder {
    ScheduleReminder {
        request_id: row.get::<Uuid, _>("request_id").to_string(),
        title: row.get("title"),
        body: row.get("body"),
        delivered_at_unix_ms: row.get("delivered_ms"),
    }
}
async fn fail(tx: &mut sqlx::PgConnection, owner: Uuid, request: Uuid) -> StorageResult<()> {
    sqlx::query("UPDATE schedules SET status='failed',claim_id=NULL,lease_until_ms=NULL WHERE user_id=$1 AND request_id=$2")
        .bind(owner).bind(request).execute(tx).await.map_err(map_error)?;
    Ok(())
}
impl ScheduleDeliveryStore for PostgresStore {
    fn claim_due_schedule(&self) -> BoxFuture<'_, StorageResult<Option<ScheduleLease>>> {
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            // 先锁用户，与批准、取消和级联删除保持一致；跳过已被其他进程锁住的用户。
            let candidate=sqlx::query("SELECT u.id,s.request_id FROM users u JOIN schedules s ON s.user_id=u.id WHERE s.run_at_ms<=floor(extract(epoch FROM clock_timestamp())*1000)::bigint AND (s.status='scheduled' OR (s.status='running' AND s.lease_until_ms<=floor(extract(epoch FROM clock_timestamp())*1000)::bigint)) ORDER BY s.run_at_ms,u.id,s.request_id FOR UPDATE OF u SKIP LOCKED LIMIT 1")
                .fetch_optional(&mut *tx).await.map_err(map_error)?;
            let Some(candidate) = candidate else {
                tx.commit().await.map_err(map_error)?;
                return Ok(None);
            };
            let owner: Uuid = candidate.get("id");
            let request: Uuid = candidate.get("request_id");
            // 锁取得前后的状态可能变化，必须重新读取并判断，不能沿用扫描快照。
            let row=sqlx::query(&format!("SELECT {FIELDS},approval,lease_until_ms FROM schedules WHERE user_id=$1 AND request_id=$2 FOR UPDATE"))
                .bind(owner).bind(request).fetch_one(&mut *tx).await.map_err(map_error)?;
            let time = now(&mut tx).await?;
            let saved = record(&row);
            if saved.run_at_unix_ms > time
                || !(saved.status == "scheduled"
                    || (saved.status == "running"
                        && row
                            .get::<Option<i64>, _>("lease_until_ms")
                            .is_some_and(|end| end <= time)))
            {
                tx.commit().await.map_err(map_error)?;
                return Ok(None);
            }
            if !authorized(owner, &row) {
                fail(&mut tx, owner, request).await?;
                tx.commit().await.map_err(map_error)?;
                return Ok(None);
            }
            let claim = Uuid::new_v4();
            let until = time + 60_000;
            sqlx::query("UPDATE schedules SET status='running',claim_id=$3,lease_until_ms=$4 WHERE user_id=$1 AND request_id=$2")
                .bind(owner).bind(request).bind(claim).bind(until).execute(&mut *tx).await.map_err(map_error)?;
            tx.commit().await.map_err(map_error)?;
            Ok(Some(ScheduleLease {
                owner: UserId::new(owner.to_string()),
                request_id: request.to_string(),
                claim_id: claim.to_string(),
                lease_until_unix_ms: until,
            }))
        })
    }
    fn deliver_schedule(
        &self,
        lease: &ScheduleLease,
    ) -> BoxFuture<'_, StorageResult<ScheduleReminder>> {
        let lease = lease.clone();
        Box::pin(async move {
            let (owner, request, claim) = (
                uuid(lease.owner.as_str())?,
                uuid(&lease.request_id)?,
                uuid(&lease.claim_id)?,
            );
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            lock_owner(&mut tx, owner).await?;
            let row=sqlx::query(&format!("SELECT {FIELDS},approval,claim_id,lease_until_ms FROM schedules WHERE user_id=$1 AND request_id=$2 FOR UPDATE"))
                .bind(owner).bind(request).fetch_one(&mut *tx).await.map_err(map_error)?;
            let saved = record(&row);
            if row.get::<Option<Uuid>, _>("claim_id") != Some(claim) {
                return Err(conflict());
            }
            if saved.status == "delivered" {
                let row=sqlx::query("SELECT request_id,title,body,delivered_ms FROM schedule_reminders WHERE user_id=$1 AND request_id=$2")
                    .bind(owner).bind(request).fetch_one(&mut *tx).await.map_err(map_error)?;
                tx.commit().await.map_err(map_error)?;
                return Ok(reminder(&row));
            }
            let time = now(&mut tx).await?;
            if saved.status != "running"
                || saved.run_at_unix_ms > time
                || row
                    .get::<Option<i64>, _>("lease_until_ms")
                    .is_none_or(|end| end <= time)
            {
                return Err(conflict());
            }
            if !authorized(owner, &row) {
                fail(&mut tx, owner, request).await?;
                tx.commit().await.map_err(map_error)?;
                return Err(conflict());
            }
            // 唯一键与终态同一事务提交；响应丢失后同一租约重放只读已有提醒。
            let row=sqlx::query("INSERT INTO schedule_reminders(user_id,request_id,title,body,delivered_ms) VALUES($1,$2,$3,$4,$5) RETURNING request_id,title,body,delivered_ms")
                .bind(owner).bind(request).bind(saved.title).bind(saved.body).bind(time).fetch_one(&mut *tx).await.map_err(map_error)?;
            sqlx::query("UPDATE schedules SET status='delivered',delivered_ms=$3 WHERE user_id=$1 AND request_id=$2")
                .bind(owner).bind(request).bind(time).execute(&mut *tx).await.map_err(map_error)?;
            tx.commit().await.map_err(map_error)?;
            Ok(reminder(&row))
        })
    }
    fn list_schedule_reminders(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ReminderPage>> {
        let owner = uuid(owner.as_str());
        let after = after.map(uuid).transpose();
        Box::pin(async move {
            let rows=sqlx::query("SELECT request_id,title,body,delivered_ms FROM schedule_reminders WHERE user_id=$1 AND ($2::uuid IS NULL OR request_id>$2) ORDER BY request_id LIMIT 21")
                .bind(owner?).bind(after?).fetch_all(&self.pool).await.map_err(map_error)?;
            let items: Vec<_> = rows.iter().take(20).map(reminder).collect();
            let next_cursor = (rows.len() > 20)
                .then(|| items.last().expect("full reminder page").request_id.clone());
            Ok(ReminderPage { items, next_cursor })
        })
    }
}
