use crate::{PostgresStore, map_error, replies, reply_money};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    replies::{PendingReply, ReplyConfiguration, ReplyContext, ReplyStatus},
    reply_budgets::{BudgetedReplyClaim, ReplyBudget, ReplyDispatchStore},
};
use sqlx::{PgConnection, Row};

fn conflict() -> StorageError {
    StorageError::Conflict("reply configuration unavailable or changed".into())
}

fn encode(budget: &ReplyBudget) -> StorageResult<String> {
    serde_json::to_string(budget)
        .map_err(|_| StorageError::InvalidData("invalid reply budget".into()))
}

async fn check(
    tx: &mut PgConnection,
    configuration: &ReplyConfiguration,
    budget: &ReplyBudget,
) -> StorageResult<()> {
    // 与停用互斥，取锁后再检查时钟，避免锁等待跨过价格有效期。
    let row = sqlx::query("SELECT model,budget::text AS budget,valid_until_ms,disabled_at IS NOT NULL AS disabled FROM reply_configurations WHERE revision=$1 FOR SHARE")
        .bind(&configuration.revision).fetch_optional(&mut *tx).await.map_err(map_error)?.ok_or_else(conflict)?;
    let now: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
            .fetch_one(tx)
            .await
            .map_err(map_error)?;
    let stored: ReplyBudget = serde_json::from_str(row.get("budget"))
        .map_err(|_| StorageError::Unavailable("invalid stored reply configuration".into()))?;
    if row.get::<bool, _>("disabled")
        || row.get::<i64, _>("valid_until_ms") <= now
        || row.get::<&str, _>("model") != configuration.model
        || &stored != budget
    {
        return Err(conflict());
    }
    Ok(())
}

impl ReplyDispatchStore for PostgresStore {
    fn pending_budgeted_replies(
        &self,
        configuration: &ReplyConfiguration,
    ) -> BoxFuture<'_, StorageResult<Vec<PendingReply>>> {
        let configuration = configuration.clone();
        Box::pin(async move {
            let rows = sqlx::query("SELECT c.user_id,r.conversation_id,r.request_id,r.status FROM conversation_replies r JOIN conversations c ON c.id=r.conversation_id JOIN reply_money_reservations b ON b.conversation_id=r.conversation_id AND b.request_id=r.request_id AND b.user_id=c.user_id WHERE c.deleted_at IS NULL AND b.model=$1 AND b.configuration_revision=$2 AND b.charged IS NULL AND (r.status='queued' OR (r.status='dispatching' AND r.dispatched_at+interval '120 seconds' <= clock_timestamp())) ORDER BY (r.status='dispatching') DESC,r.created_at,r.conversation_id,r.request_id LIMIT 20")
                .bind(configuration.model).bind(configuration.revision).fetch_all(&self.pool).await.map_err(map_error)?;
            Ok(rows
                .iter()
                .map(|row| PendingReply {
                    owner: UserId::new(row.get::<uuid::Uuid, _>("user_id").to_string()),
                    conversation: row.get::<uuid::Uuid, _>("conversation_id").to_string(),
                    request: row.get::<uuid::Uuid, _>("request_id").to_string(),
                    status: if row.get::<&str, _>("status") == "queued" {
                        ReplyStatus::Queued
                    } else {
                        ReplyStatus::Dispatching
                    },
                })
                .collect())
        })
    }

    fn register_reply_configuration(
        &self,
        configuration: &ReplyConfiguration,
        budget: &ReplyBudget,
        valid_until_unix_ms: i64,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let configuration = configuration.clone();
        let budget = budget.clone();
        Box::pin(async move {
            if [&configuration.model, &configuration.revision]
                .iter()
                .any(|v| v.trim().is_empty() || v.len() > 128 || v.contains('\0'))
            {
                return Err(StorageError::InvalidData(
                    "invalid reply configuration".into(),
                ));
            }
            let output = u32::try_from(budget.output_token_bound)
                .map_err(|_| StorageError::InvalidData("invalid output bound".into()))?;
            let (_, quote) = reply_money::validate(
                budget.clone(),
                &ReplyContext {
                    system: String::new(),
                    user_messages: vec![],
                    first_sequence: 1,
                    max_output_tokens: output,
                    configuration: configuration.clone(),
                },
            )?;
            if budget.daily_limit < quote.amount() {
                return Err(StorageError::InvalidData("invalid daily limit".into()));
            }
            let json = encode(&budget)?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            sqlx::query("INSERT INTO reply_configurations(revision,model,budget,valid_until_ms) SELECT $1,$2,$3::text::jsonb,$4 WHERE $4 > floor(extract(epoch FROM clock_timestamp())*1000)::bigint ON CONFLICT DO NOTHING")
                .bind(&configuration.revision).bind(&configuration.model).bind(json).bind(valid_until_unix_ms).execute(&mut *tx).await.map_err(map_error)?;
            check(&mut tx, &configuration, &budget).await?;
            let until: i64 = sqlx::query_scalar(
                "SELECT valid_until_ms FROM reply_configurations WHERE revision=$1",
            )
            .bind(&configuration.revision)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
            if until != valid_until_unix_ms {
                return Err(conflict());
            }
            tx.commit().await.map_err(map_error)
        })
    }

    fn disable_reply_configuration(&self, revision: &str) -> BoxFuture<'_, StorageResult<()>> {
        let revision = revision.to_owned();
        Box::pin(async move {
            let result = sqlx::query("UPDATE reply_configurations SET disabled_at=COALESCE(disabled_at,clock_timestamp()) WHERE revision=$1")
                .bind(revision).execute(&self.pool).await.map_err(map_error)?;
            if result.rows_affected() != 1 {
                return Err(StorageError::NotFound);
            }
            Ok(())
        })
    }

    fn check_reply_configuration(
        &self,
        configuration: &ReplyConfiguration,
        budget: &ReplyBudget,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let configuration = configuration.clone();
        let budget = budget.clone();
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            check(&mut tx, &configuration, &budget).await?;
            tx.commit().await.map_err(map_error)
        })
    }

    fn claim_budgeted_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<BudgetedReplyClaim>> {
        let ids = replies::ids(owner, conversation, request);
        Box::pin(async move {
            let (owner, conversation, request) = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            replies::lock(&mut tx, owner, conversation).await?;
            let mut reply = replies::read(&mut tx, conversation, request).await?;
            if reply.status != ReplyStatus::Queued {
                return Err(StorageError::Conflict(
                    "reply already claimed or terminal".into(),
                ));
            }
            let context = reply.context.as_ref().ok_or_else(conflict)?;
            let row = sqlx::query("SELECT model,configuration_revision,budget::text AS budget,reserved FROM reply_money_reservations WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3 AND charged IS NULL FOR UPDATE")
                .bind(owner).bind(conversation).bind(request).fetch_optional(&mut *tx).await.map_err(map_error)?.ok_or_else(conflict)?;
            let budget: ReplyBudget = serde_json::from_str(row.get("budget"))
                .map_err(|_| StorageError::Unavailable("invalid stored reply budget".into()))?;
            let (budget, quote) = reply_money::validate(budget, context)?;
            let reserved: i64 = row.get("reserved");
            if quote.amount() != reserved
                || row.get::<&str, _>("model") != context.configuration.model
                || row.get::<&str, _>("configuration_revision") != context.configuration.revision
            {
                return Err(conflict());
            }
            check(&mut tx, &context.configuration, &budget).await?;
            sqlx::query("UPDATE conversation_replies SET status='dispatching',dispatched_at=clock_timestamp() WHERE conversation_id=$1 AND request_id=$2")
                .bind(conversation).bind(request).execute(&mut *tx).await.map_err(map_error)?;
            tx.commit().await.map_err(map_error)?;
            reply.status = ReplyStatus::Dispatching;
            Ok(BudgetedReplyClaim {
                reply,
                budget,
                reserved,
            })
        })
    }
}
