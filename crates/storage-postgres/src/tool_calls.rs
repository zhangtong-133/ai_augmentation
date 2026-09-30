use crate::{PostgresStore, map_error};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    dates::validate_utc_day,
    tool_calls::{
        DAILY_TOOL_CALL_LIMIT, NewToolCall, ToolCall, ToolCallAudit, ToolCallFinish, ToolCallStart,
        ToolCallStore, validate_new_tool_call, validate_tool_call_finish,
    },
};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

const FIELDS: &str = "request_id,tool,day::text,CASE WHEN status='running' AND deadline<=clock_timestamp() THEN 'unknown' ELSE status END AS status,input_bytes,output_bytes,floor(extract(epoch FROM created_at)*1000)::bigint AS created_ms,floor(extract(epoch FROM deadline)*1000)::bigint AS deadline_ms,floor(extract(epoch FROM finished_at)*1000)::bigint AS finished_ms";

fn uuid(value: &str) -> StorageResult<Uuid> {
    Uuid::parse_str(value).map_err(|_| StorageError::InvalidData("invalid tool call id".into()))
}

fn record(row: &PgRow) -> ToolCall {
    ToolCall {
        request_id: row.get::<Uuid, _>("request_id").to_string(),
        tool: row.get("tool"),
        day: row.get("day"),
        status: row.get("status"),
        input_bytes: row.get("input_bytes"),
        output_bytes: row.get("output_bytes"),
        created_at_unix_ms: row.get("created_ms"),
        deadline_unix_ms: row.get("deadline_ms"),
        finished_at_unix_ms: row.get("finished_ms"),
    }
}

impl ToolCallStore for PostgresStore {
    fn start_tool_call(
        &self,
        owner: &UserId,
        call: &NewToolCall,
    ) -> BoxFuture<'_, StorageResult<ToolCallStart>> {
        let ids = uuid(owner.as_str()).and_then(|owner| Ok((owner, uuid(&call.request_id)?)));
        let call = call.clone();
        Box::pin(async move {
            validate_new_tool_call(&call)?;
            let (owner, id) = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
                .bind(owner)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_error)?;
            let result = start_locked(&mut tx, owner, id, &call).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(result)
        })
    }

    fn finish_tool_call(
        &self,
        owner: &UserId,
        request_id: &str,
        finish: ToolCallFinish,
    ) -> BoxFuture<'_, StorageResult<ToolCall>> {
        let ids = uuid(owner.as_str()).and_then(|owner| Ok((owner, uuid(request_id)?)));
        Box::pin(async move {
            validate_tool_call_finish(finish)?;
            let (owner, id) = ids?;
            // 原 running 记录可在结果晚到时补齐；unknown 只是读时状态，不触发重试。
            let row = sqlx::query(&format!(
                "UPDATE tool_calls SET status=$3,output_bytes=$4,finished_at=COALESCE(finished_at,clock_timestamp()) WHERE user_id=$1 AND request_id=$2 AND (status='running' OR (status=$3 AND output_bytes IS NOT DISTINCT FROM $4)) RETURNING {FIELDS}"
            )).bind(owner).bind(id).bind(finish.outcome.as_str()).bind(finish.output_bytes)
                .fetch_optional(&self.pool).await.map_err(map_error)?;
            if let Some(row) = row {
                return Ok(record(&row));
            }
            self.get_tool_call(&UserId::new(owner.to_string()), &id.to_string())
                .await?;
            Err(StorageError::Conflict("tool call already finished".into()))
        })
    }

    fn get_tool_call(
        &self,
        owner: &UserId,
        request_id: &str,
    ) -> BoxFuture<'_, StorageResult<ToolCall>> {
        let ids = uuid(owner.as_str()).and_then(|owner| Ok((owner, uuid(request_id)?)));
        Box::pin(async move {
            let (owner, id) = ids?;
            let row = sqlx::query(&format!(
                "SELECT {FIELDS} FROM tool_calls WHERE user_id=$1 AND request_id=$2"
            ))
            .bind(owner)
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map_err(map_error)?;
            Ok(record(&row))
        })
    }

    fn audit_tool_calls(
        &self,
        owner: &UserId,
        day: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ToolCallAudit>> {
        let owner = uuid(owner.as_str());
        let day = day.map(str::to_owned);
        Box::pin(async move {
            if let Some(day) = &day {
                validate_utc_day(day)?;
            }
            let owner = owner?;
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
            let day: String = sqlx::query_scalar(
                "SELECT COALESCE($1::text::date,(clock_timestamp() AT TIME ZONE 'UTC')::date)::text"
            ).bind(day).fetch_one(&mut *tx).await.map_err(map_error)?;
            let used: i32 = sqlx::query_scalar(
                "SELECT COALESCE((SELECT used FROM tool_daily_budgets WHERE user_id=$1 AND day=$2::text::date),0)"
            ).bind(owner).bind(&day).fetch_one(&mut *tx).await.map_err(map_error)?;
            let rows = sqlx::query(&format!(
                "SELECT {FIELDS} FROM tool_calls WHERE user_id=$1 AND day=$2::text::date ORDER BY created_at,request_id LIMIT 100"
            )).bind(owner).bind(&day).fetch_all(&mut *tx).await.map_err(map_error)?;
            tx.commit().await.map_err(map_error)?;
            Ok(ToolCallAudit {
                day,
                used,
                limit: DAILY_TOOL_CALL_LIMIT,
                items: rows.iter().map(record).collect(),
            })
        })
    }
}

// 调用方必须已持有用户行锁；计划领取与工具日预算在同一个事务内提交。
pub(super) async fn start_locked(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner: Uuid,
    id: Uuid,
    call: &NewToolCall,
) -> StorageResult<ToolCallStart> {
    if let Some(row) = sqlx::query(&format!(
        "SELECT {FIELDS},arguments_digest FROM tool_calls WHERE user_id=$1 AND request_id=$2"
    ))
    .bind(owner)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(map_error)?
    {
        if row.get::<String, _>("tool") != call.tool
            || row.get::<String, _>("arguments_digest") != call.arguments_digest
            || row.get::<i32, _>("input_bytes") != call.input_bytes
        {
            return Err(StorageError::Conflict(
                "tool call request already used".into(),
            ));
        }
        return Ok(ToolCallStart::Existing(record(&row)));
    }
    // 在用户锁之后取 UTC 日，避免跨午夜等待锁导致额度记入旧日期。
    let day: String =
        sqlx::query_scalar("SELECT (clock_timestamp() AT TIME ZONE 'UTC')::date::text")
            .fetch_one(&mut **tx)
            .await
            .map_err(map_error)?;
    sqlx::query("INSERT INTO tool_daily_budgets(user_id,day,used) VALUES($1,$2::text::date,0) ON CONFLICT DO NOTHING")
        .bind(owner).bind(&day).execute(&mut **tx).await.map_err(map_error)?;
    let used: Option<i32> = sqlx::query_scalar(
        "UPDATE tool_daily_budgets SET used=used+1 WHERE user_id=$1 AND day=$2::text::date AND used<$3 RETURNING used"
    ).bind(owner).bind(&day).bind(DAILY_TOOL_CALL_LIMIT)
        .fetch_optional(&mut **tx).await.map_err(map_error)?;
    if used.is_none() {
        return Err(StorageError::Conflict("tool call quota reached".into()));
    }
    let row = sqlx::query(&format!(
        "INSERT INTO tool_calls(user_id,request_id,tool,arguments_digest,day,input_bytes) VALUES($1,$2,$3,$4,$5::text::date,$6) RETURNING {FIELDS}"
    )).bind(owner).bind(id).bind(&call.tool).bind(&call.arguments_digest).bind(day)
        .bind(call.input_bytes).fetch_one(&mut **tx).await.map_err(map_error)?;
    Ok(ToolCallStart::Started(record(&row)))
}
