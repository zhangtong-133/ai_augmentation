use crate::{PostgresStore, map_error};
use personal_ai_agent_core::reply::plan_reply;
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    messages::{Message, MessageSnapshot},
    replies::{
        PendingReply, Reply, ReplyConfiguration, ReplyContext, ReplyOutcome, ReplyStatus,
        ReplyStore,
    },
};
use sqlx::{PgConnection, Row, postgres::PgRow};
use uuid::Uuid;

fn ids(owner: &UserId, conversation: &str, request: &str) -> StorageResult<(Uuid, Uuid, Uuid)> {
    let parse = |value: &str| {
        Uuid::parse_str(value).map_err(|_| StorageError::InvalidData("invalid reply id".into()))
    };
    Ok((
        parse(owner.as_str())?,
        parse(conversation)?,
        parse(request)?,
    ))
}

fn record(row: &PgRow) -> StorageResult<Reply> {
    let status = match row.get::<&str, _>("status") {
        "queued" => ReplyStatus::Queued,
        "dispatching" => ReplyStatus::Dispatching,
        "succeeded" => ReplyStatus::Succeeded,
        "failed" => ReplyStatus::Failed,
        "unknown" => ReplyStatus::Unknown,
        "cancelled" => ReplyStatus::Cancelled,
        _ => return Err(StorageError::Unavailable("invalid reply status".into())),
    };
    let context = row
        .get::<Option<String>, _>("context_text")
        .map(|text| serde_json::from_str(&text))
        .transpose()
        .map_err(|_| StorageError::Unavailable("invalid reply context".into()))?;
    Ok(Reply {
        request_id: row.get::<Uuid, _>("request_id").to_string(),
        revision: row.get("revision"),
        status,
        context,
        output: row.get("output"),
    })
}

async fn lock(tx: &mut PgConnection, owner: Uuid, conversation: Uuid) -> StorageResult<i64> {
    // 所有回复写入与删除采用同一锁序：用户、对话、回复。
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(owner)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_error)?;
    sqlx::query_scalar("SELECT message_revision FROM conversations WHERE user_id=$1 AND id=$2 AND deleted_at IS NULL FOR UPDATE")
        .bind(owner).bind(conversation).fetch_one(&mut *tx).await.map_err(map_error)
}

async fn read(tx: &mut PgConnection, conversation: Uuid, request: Uuid) -> StorageResult<Reply> {
    let row = sqlx::query("SELECT *,context::text AS context_text FROM conversation_replies WHERE conversation_id=$1 AND request_id=$2")
        .bind(conversation).bind(request).fetch_one(tx).await.map_err(map_error)?;
    record(&row)
}

enum Action {
    Claim,
    Cancel,
    Finish(ReplyOutcome),
    Expire,
}

impl PostgresStore {
    fn change_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        action: Action,
    ) -> BoxFuture<'_, StorageResult<Reply>> {
        let ids = ids(owner, conversation, request);
        Box::pin(async move {
            let (owner, conversation, request) = ids?;
            if let Action::Finish(ReplyOutcome::Succeeded(text)) = &action
                && (text.trim().is_empty() || text.len() > 16384 || text.contains('\0'))
            {
                return Err(StorageError::InvalidData("invalid reply output".into()));
            }
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let result = async {
                lock(&mut tx, owner, conversation).await?;
                let reply = read(&mut tx, conversation, request).await?;
                match action {
                    Action::Claim => {
                        if reply.status != ReplyStatus::Queued { return Err(StorageError::Conflict("reply already claimed or terminal".into())); }
                        sqlx::query("UPDATE conversation_replies SET status='dispatching',dispatched_at=clock_timestamp() WHERE conversation_id=$1 AND request_id=$2")
                            .bind(conversation).bind(request).execute(&mut *tx).await.map_err(map_error)?;
                    }
                    Action::Cancel => {
                        if reply.status == ReplyStatus::Queued {
                            sqlx::query("UPDATE reply_daily_budgets SET reserved=reserved-1 WHERE user_id=$1 AND day=(SELECT budget_day FROM conversation_replies WHERE conversation_id=$2 AND request_id=$3)")
                                .bind(owner).bind(conversation).bind(request).execute(&mut *tx).await.map_err(map_error)?;
                        }
                        sqlx::query("UPDATE conversation_replies SET status='cancelled',context=NULL WHERE conversation_id=$1 AND request_id=$2 AND status IN ('queued','dispatching')")
                            .bind(conversation).bind(request).execute(&mut *tx).await.map_err(map_error)?;
                    }
                    Action::Finish(outcome) => {
                        if reply.status == ReplyStatus::Queued { return Err(StorageError::Conflict("reply not claimed".into())); }
                        let (status, output) = match outcome {
                            ReplyOutcome::Succeeded(text) => ("succeeded", Some(text)),
                            ReplyOutcome::Failed => ("failed", None),
                            ReplyOutcome::Unknown => ("unknown", None),
                        };
                        // 过期和取消结果不能写回。重复完成只返回已持久化状态。
                        sqlx::query("UPDATE conversation_replies SET status=CASE WHEN dispatched_at+interval '120 seconds' <= statement_timestamp() THEN 'unknown' ELSE $3 END,output=CASE WHEN dispatched_at+interval '120 seconds' > statement_timestamp() THEN $4 ELSE NULL END,context=NULL WHERE conversation_id=$1 AND request_id=$2 AND status='dispatching'")
                            .bind(conversation).bind(request).bind(status).bind(output).execute(&mut *tx).await.map_err(map_error)?;
                    }
                    Action::Expire => {
                        sqlx::query("UPDATE conversation_replies SET status='unknown',context=NULL WHERE conversation_id=$1 AND request_id=$2 AND status='dispatching' AND dispatched_at+interval '120 seconds' <= clock_timestamp()")
                            .bind(conversation).bind(request).execute(&mut *tx).await.map_err(map_error)?;
                    }
                }
                read(&mut tx, conversation, request).await
            }.await;
            if result.is_ok() {
                tx.commit().await.map_err(map_error)?;
            } else {
                tx.rollback().await.map_err(map_error)?;
            }
            result
        })
    }
}

impl ReplyStore for PostgresStore {
    fn list_replies(
        &self,
        owner: &UserId,
        conversation: &str,
    ) -> BoxFuture<'_, StorageResult<Vec<Reply>>> {
        let ids = ids(owner, conversation, &Uuid::nil().to_string());
        Box::pin(async move {
            let (owner, conversation, _) = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let result = async {
                // 与删除互斥，空列表也不能绕过归属检查。
                sqlx::query("SELECT id FROM conversations WHERE user_id=$1 AND id=$2 AND deleted_at IS NULL FOR SHARE")
                    .bind(owner).bind(conversation).fetch_one(&mut *tx).await.map_err(map_error)?;
                let rows = sqlx::query("SELECT request_id,revision,status,output,NULL::text AS context_text FROM conversation_replies WHERE conversation_id=$1 ORDER BY created_at,request_id LIMIT 100")
                    .bind(conversation).fetch_all(&mut *tx).await.map_err(map_error)?;
                rows.iter().map(record).collect::<StorageResult<Vec<_>>>()
            }.await;
            if result.is_ok() {
                tx.commit().await.map_err(map_error)?;
            } else {
                tx.rollback().await.map_err(map_error)?;
            }
            result
        })
    }
    fn pending_replies(&self) -> BoxFuture<'_, StorageResult<Vec<PendingReply>>> {
        Box::pin(async move {
            let rows = sqlx::query("SELECT c.user_id,r.conversation_id,r.request_id,r.status FROM conversation_replies r JOIN conversations c ON c.id=r.conversation_id WHERE c.deleted_at IS NULL AND (r.status='queued' OR (r.status='dispatching' AND r.dispatched_at+interval '120 seconds' <= clock_timestamp())) ORDER BY (r.status='dispatching') DESC,r.created_at,r.conversation_id,r.request_id LIMIT 20")
                .fetch_all(&self.pool).await.map_err(map_error)?;
            Ok(rows
                .iter()
                .map(|row| PendingReply {
                    owner: UserId::new(row.get::<Uuid, _>("user_id").to_string()),
                    conversation: row.get::<Uuid, _>("conversation_id").to_string(),
                    request: row.get::<Uuid, _>("request_id").to_string(),
                    status: if row.get::<&str, _>("status") == "queued" {
                        ReplyStatus::Queued
                    } else {
                        ReplyStatus::Dispatching
                    },
                })
                .collect())
        })
    }
    fn reserve_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        revision: i64,
        configuration: &ReplyConfiguration,
    ) -> BoxFuture<'_, StorageResult<Reply>> {
        let ids = ids(owner, conversation, request);
        let configuration = configuration.clone();
        Box::pin(async move {
            let (owner, conversation, request) = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let result = async {
                let current = lock(&mut tx, owner, conversation).await?;
                if let Some(row) = sqlx::query("SELECT *,context::text AS context_text FROM conversation_replies WHERE conversation_id=$1 AND request_id=$2")
                    .bind(conversation).bind(request).fetch_optional(&mut *tx).await.map_err(map_error)? {
                    let reply = record(&row)?;
                    if reply.revision != revision { return Err(StorageError::Conflict("reply request already used".into())); }
                    return Ok(reply);
                }
                for value in [&configuration.model, &configuration.revision] {
                    if value.trim().is_empty() || value.len() > 128 || value.contains('\0') { return Err(StorageError::InvalidData("invalid reply configuration".into())); }
                }
                if current != revision { return Err(StorageError::Conflict("stale reply revision".into())); }
                let count: i64 = sqlx::query_scalar("SELECT count(*) FROM conversation_replies WHERE conversation_id=$1")
                    .bind(conversation).fetch_one(&mut *tx).await.map_err(map_error)?;
                let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM conversation_replies WHERE conversation_id=$1 AND status IN ('queued','dispatching'))")
                    .bind(conversation).fetch_one(&mut *tx).await.map_err(map_error)?;
                if count >= 100 || active { return Err(StorageError::Conflict("reply quota reached".into())); }
                let rows = sqlx::query("SELECT id,sequence,content,(extract(epoch FROM created_at)*1000)::bigint AS created_ms FROM conversation_messages WHERE conversation_id=$1 ORDER BY sequence")
                    .bind(conversation).fetch_all(&mut *tx).await.map_err(map_error)?;
                let snapshot = MessageSnapshot { revision: current, deleted: false, messages: rows.iter().map(|row| Message {
                    id: row.get::<Uuid,_>("id").to_string(), sequence: row.get("sequence"), content: row.get("content"), created_at_unix_ms: row.get("created_ms"),
                }).collect() };
                let plan = plan_reply(&snapshot, revision).map_err(|_| StorageError::InvalidData("invalid reply snapshot".into()))?;
                let context = ReplyContext { system: plan.request.messages[0].content.clone(), user_messages: plan.request.messages[1..].iter().map(|message| message.content.clone()).collect(), first_sequence: plan.first_sequence, max_output_tokens: plan.request.max_output_tokens.unwrap_or(1024), configuration };
                let context = serde_json::to_string(&context).map_err(|_| StorageError::InvalidData("invalid reply context".into()))?;
                // 在取得用户锁之后读取时钟；跨 UTC 午夜等待不会使用事务开始时的旧日期。
                let day: String = sqlx::query_scalar("SELECT (clock_timestamp() AT TIME ZONE 'UTC')::date::text").fetch_one(&mut *tx).await.map_err(map_error)?;
                let reserved = sqlx::query("INSERT INTO reply_daily_budgets(user_id,day,reserved) VALUES($1,$2::text::date,1) ON CONFLICT(user_id,day) DO UPDATE SET reserved=reply_daily_budgets.reserved+1 WHERE reply_daily_budgets.reserved < 20")
                    .bind(owner).bind(&day).execute(&mut *tx).await.map_err(map_error)?;
                if reserved.rows_affected() == 0 { return Err(StorageError::Conflict("daily reply quota reached".into())); }
                sqlx::query("INSERT INTO conversation_replies(conversation_id,request_id,revision,budget_day,status,context) VALUES($1,$2,$3,$4::text::date,'queued',$5::text::jsonb)")
                    .bind(conversation).bind(request).bind(revision).bind(day).bind(context).execute(&mut *tx).await.map_err(map_error)?;
                read(&mut tx, conversation, request).await
            }.await;
            if result.is_ok() {
                tx.commit().await.map_err(map_error)?;
            } else {
                tx.rollback().await.map_err(map_error)?;
            }
            result
        })
    }
    fn get_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Reply>> {
        let ids = ids(owner, conversation, request);
        Box::pin(async move {
            let (owner, conversation, request) = ids?;
            let row = sqlx::query("SELECT r.*,r.context::text AS context_text FROM conversation_replies r JOIN conversations c ON c.id=r.conversation_id WHERE c.user_id=$1 AND c.id=$2 AND c.deleted_at IS NULL AND r.request_id=$3")
                .bind(owner).bind(conversation).bind(request).fetch_one(&self.pool).await.map_err(map_error)?;
            record(&row)
        })
    }
    fn claim_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Reply>> {
        self.change_reply(owner, conversation, request, Action::Claim)
    }
    fn cancel_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Reply>> {
        self.change_reply(owner, conversation, request, Action::Cancel)
    }
    fn finish_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        outcome: ReplyOutcome,
    ) -> BoxFuture<'_, StorageResult<Reply>> {
        self.change_reply(owner, conversation, request, Action::Finish(outcome))
    }
    fn expire_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Reply>> {
        self.change_reply(owner, conversation, request, Action::Expire)
    }
}
