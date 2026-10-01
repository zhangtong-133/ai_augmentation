//! 付费规划首阶段：预览不扣款，精确批准后事务预留；不调用供应商。
use crate::{PostgresStore, map_error, replies, reply_money};
use personal_ai_agent_core::model_plan::{
    AgentBudgetUsage, AgentRequestIdentity, MAX_QUOTE_LIFETIME_MS, MODEL_PLAN_VERSION,
    ModelPlanningQuote, QuoteWindow, decode_model_searches, quote_model_planning,
    quote_model_planning_budget,
};
use personal_ai_domain::{ConversationId, UserId};
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    messages::{Message, MessageSnapshot},
    model_agents::{
        AgentCallCounts, AgentQuoteApproval, ModelPlanningAuthorization, ModelPlanningClaim,
        ModelPlanningConfiguration, ModelPlanningOutcome, ModelPlanningRequest, ModelPlanningStore,
        NewModelPlanningRequest,
    },
    reply_budgets::{ReplyBudget, ReplyUsage},
};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

type Keys = (Uuid, Uuid, Uuid);
const FIELDS: &str = "*,configuration::text AS config_text,snapshot::text AS snapshot_text,searches::text AS searches_text,budget_day::text AS day_text";

fn invalid() -> StorageError {
    StorageError::InvalidData("invalid model planning input".into())
}
fn conflict() -> StorageError {
    StorageError::Conflict("model planning request unavailable or mismatched".into())
}
fn corrupt() -> StorageError {
    StorageError::Unavailable("inconsistent stored model planning request".into())
}
fn keys(owner: &UserId, conversation: &str, request: &str) -> StorageResult<Keys> {
    Ok((
        Uuid::parse_str(owner.as_str()).map_err(|_| invalid())?,
        Uuid::parse_str(conversation).map_err(|_| invalid())?,
        Uuid::parse_str(request).map_err(|_| invalid())?,
    ))
}
fn encode(value: &impl serde::Serialize) -> StorageResult<String> {
    serde_json::to_string(value).map_err(|_| invalid())
}
async fn now(tx: &mut PgConnection) -> StorageResult<i64> {
    sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
        .fetch_one(tx)
        .await
        .map_err(map_error)
}
fn window(now: i64, configuration: &ModelPlanningConfiguration) -> StorageResult<QuoteWindow> {
    Ok(QuoteWindow {
        now_unix_ms: now,
        expires_at_unix_ms: now
            .checked_add(MAX_QUOTE_LIFETIME_MS)
            .ok_or_else(invalid)?
            .min(configuration.budget.valid_until_unix_ms),
    })
}
fn money_budget(configuration: &ModelPlanningConfiguration) -> ReplyBudget {
    let budget = &configuration.budget;
    ReplyBudget {
        currency: budget.currency.clone(),
        provider: budget.provider.clone(),
        price_version: budget.price_version.clone(),
        counter_version: budget.counter_version.clone(),
        input_price_per_million: budget.input_price_per_million,
        output_price_per_million: budget.output_price_per_million,
        input_token_bound: budget.input_token_bound,
        output_token_bound: budget.output_token_bound,
        request_limit: configuration.limits.phase_amount,
        daily_limit: configuration.limits.daily_amount,
    }
}

async fn check_configuration(
    tx: &mut PgConnection,
    configuration: &ModelPlanningConfiguration,
) -> StorageResult<()> {
    let row = sqlx::query("SELECT configuration::text AS config,valid_until_ms,disabled_at IS NULL AND valid_until_ms>floor(extract(epoch FROM clock_timestamp())*1000)::bigint AS active FROM model_planning_configurations WHERE version=$1 FOR SHARE")
        .bind(&configuration.budget.configuration_version).fetch_optional(tx).await.map_err(map_error)?.ok_or_else(conflict)?;
    let stored: ModelPlanningConfiguration =
        serde_json::from_str(row.get("config")).map_err(|_| corrupt())?;
    if !row.get::<bool, _>("active")
        || stored != *configuration
        || row.get::<i64, _>("valid_until_ms") != configuration.budget.valid_until_unix_ms
    {
        return Err(conflict());
    }
    Ok(())
}

struct Stored {
    request: ModelPlanningRequest,
    configuration: ModelPlanningConfiguration,
    snapshot: Option<MessageSnapshot>,
    approval: Option<Value>,
    claim_id: Option<Uuid>,
    day: Option<String>,
}
async fn read(tx: &mut PgConnection, ids: Keys) -> StorageResult<Stored> {
    let row = sqlx::query(&format!("SELECT {FIELDS} FROM model_planning_requests WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3"))
        .bind(ids.0).bind(ids.1).bind(ids.2).fetch_one(tx).await.map_err(map_error)?;
    let searches = row
        .get::<Option<String>, _>("searches_text")
        .map(|text| {
            decode_model_searches(text.as_bytes())
                .map(|p| p.searches().to_vec())
                .map_err(|_| corrupt())
        })
        .transpose()?;
    Ok(Stored {
        request: ModelPlanningRequest {
            request_id: ids.2.to_string(),
            conversation_id: ids.1.to_string(),
            revision: row.get("revision"),
            version: row.get("version"),
            digest: row.get("digest"),
            status: row.get("status"),
            currency: row.get("currency"),
            amount: row.get("amount"),
            calls: AgentCallCounts {
                chat: 1,
                embedding: 0,
                tool: 0,
            },
            searches,
            created_at_unix_ms: row.get("created_at_ms"),
            expires_at_unix_ms: row.get("expires_at_ms"),
            approved_at_unix_ms: row.get("approved_at_ms"),
        },
        configuration: serde_json::from_str(row.get("config_text")).map_err(|_| corrupt())?,
        snapshot: row
            .get::<Option<String>, _>("snapshot_text")
            .map(|text| serde_json::from_str(&text).map_err(|_| corrupt()))
            .transpose()?,
        approval: row.get("approval"),
        claim_id: row.get("claim_id"),
        day: row.get("day_text"),
    })
}
fn frozen(ids: Keys, stored: &Stored) -> StorageResult<ModelPlanningQuote> {
    if stored.request.version != MODEL_PLAN_VERSION {
        return Err(conflict());
    }
    let quote = quote_model_planning(
        AgentRequestIdentity {
            owner: UserId::new(ids.0.to_string()),
            conversation: ConversationId::new(ids.1.to_string()),
            request_id: ids.2.to_string(),
        },
        stored.snapshot.as_ref().ok_or_else(corrupt)?,
        stored.request.revision,
        stored.configuration.budget.clone(),
        stored.configuration.limits,
        QuoteWindow {
            now_unix_ms: stored.request.created_at_unix_ms,
            expires_at_unix_ms: stored.request.expires_at_unix_ms,
        },
    )
    .map_err(|_| corrupt())?;
    if quote.quote().digest() != stored.request.digest
        || quote.quote().amount() != stored.request.amount
        || quote.quote().currency() != stored.request.currency
    {
        return Err(corrupt());
    }
    Ok(quote)
}
async fn check_receipt(tx: &mut PgConnection, ids: Keys, stored: &Stored) -> StorageResult<()> {
    let row = sqlx::query("SELECT budget::text AS budget,reserved,currency,model,configuration_revision,day::text AS day FROM reply_money_reservations WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3 AND request_kind='model_planning' AND charged IS NULL FOR UPDATE")
        .bind(ids.0).bind(ids.1).bind(ids.2).fetch_optional(&mut *tx).await.map_err(map_error)?.ok_or_else(corrupt)?;
    let budget: ReplyBudget = serde_json::from_str(row.get("budget")).map_err(|_| corrupt())?;
    if stored.day.as_deref() != Some(row.get::<&str, _>("day"))
        || budget != money_budget(&stored.configuration)
        || row.get::<i64, _>("reserved") != stored.request.amount
        || row.get::<&str, _>("currency") != stored.request.currency
        || row.get::<&str, _>("model") != stored.configuration.budget.model
        || row.get::<&str, _>("configuration_revision")
            != stored.configuration.budget.configuration_version
    {
        return Err(corrupt());
    }
    let valid: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM model_agent_daily m JOIN reply_money_daily b USING(user_id,day) WHERE m.user_id=$1 AND m.day=$2::text::date AND m.occupied>=1 AND b.currency=$3 AND b.occupied>=$4)")
        .bind(ids.0).bind(row.get::<&str,_>("day")).bind(&stored.request.currency).bind(stored.request.amount).fetch_one(tx).await.map_err(map_error)?;
    if !valid {
        return Err(corrupt());
    }
    Ok(())
}

async fn terminate(
    tx: &mut PgConnection,
    ids: Keys,
    stored: &Stored,
    status: &str,
    searches: Option<Value>,
    usage: Option<ReplyUsage>,
) -> StorageResult<()> {
    if matches!(stored.request.status.as_str(), "queued" | "dispatching") {
        check_receipt(tx, ids, stored).await?;
        let before = stored.request.status == "queued";
        reply_money::settle_kind(tx, ids, "model_planning", before, usage).await?;
        if before {
            let updated = sqlx::query("UPDATE model_agent_daily SET occupied=occupied-1 WHERE user_id=$1 AND day=(SELECT budget_day FROM model_planning_requests WHERE user_id=$1 AND request_id=$2) AND occupied>=1")
                .bind(ids.0).bind(ids.2).execute(&mut *tx).await.map_err(map_error)?;
            if updated.rows_affected() != 1 {
                return Err(corrupt());
            }
        }
    }
    sqlx::query("UPDATE model_planning_requests SET status=$4,searches=$5,snapshot=CASE WHEN $4='succeeded' THEN snapshot ELSE NULL END WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3")
        .bind(ids.0).bind(ids.1).bind(ids.2).bind(status).bind(searches).execute(tx).await.map_err(map_error)?;
    Ok(())
}
async fn expire(tx: &mut PgConnection, owner: Uuid, conversation: Uuid) -> StorageResult<()> {
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT request_id FROM model_planning_requests WHERE user_id=$1 AND conversation_id=$2 AND ((status IN ('draft','queued') AND expires_at_ms<=floor(extract(epoch FROM clock_timestamp())*1000)::bigint) OR (status='dispatching' AND deadline_ms<=floor(extract(epoch FROM clock_timestamp())*1000)::bigint)) ORDER BY request_id LIMIT 100")
        .bind(owner).bind(conversation).fetch_all(&mut *tx).await.map_err(map_error)?;
    for request in ids {
        let ids = (owner, conversation, request);
        let stored = read(tx, ids).await?;
        let status = if stored.request.status == "dispatching" {
            "unknown"
        } else {
            "expired"
        };
        terminate(tx, ids, &stored, status, None, None).await?;
    }
    Ok(())
}

pub(super) async fn delete_conversation(
    tx: &mut PgConnection,
    owner: Uuid,
    conversation: Uuid,
) -> StorageResult<()> {
    let requests: Vec<Uuid> = sqlx::query_scalar("SELECT request_id FROM model_planning_requests WHERE user_id=$1 AND conversation_id=$2 AND status IN ('queued','dispatching')")
        .bind(owner).bind(conversation).fetch_all(&mut *tx).await.map_err(map_error)?;
    for request in requests {
        let ids = (owner, conversation, request);
        let stored = read(tx, ids).await?;
        terminate(tx, ids, &stored, "cancelled", None, None).await?;
    }
    sqlx::query("DELETE FROM model_planning_requests WHERE user_id=$1 AND conversation_id=$2")
        .bind(owner)
        .bind(conversation)
        .execute(tx)
        .await
        .map_err(map_error)?;
    Ok(())
}

impl ModelPlanningStore for PostgresStore {
    fn register_model_planning_configuration(
        &self,
        configuration: &ModelPlanningConfiguration,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let configuration = configuration.clone();
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let time = now(&mut tx).await?;
            quote_model_planning_budget(
                &configuration.budget,
                configuration.limits,
                window(time, &configuration)?,
            )
            .map_err(|_| invalid())?;
            sqlx::query("INSERT INTO model_planning_configurations(version,configuration,valid_until_ms) VALUES($1,$2::text::jsonb,$3) ON CONFLICT DO NOTHING")
                .bind(&configuration.budget.configuration_version).bind(encode(&configuration)?).bind(configuration.budget.valid_until_unix_ms).execute(&mut *tx).await.map_err(map_error)?;
            check_configuration(&mut tx, &configuration).await?;
            tx.commit().await.map_err(map_error)
        })
    }
    fn disable_model_planning_configuration(
        &self,
        version: &str,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let version = version.to_owned();
        Box::pin(async move {
            let updated = sqlx::query("UPDATE model_planning_configurations SET disabled_at=COALESCE(disabled_at,clock_timestamp()) WHERE version=$1")
                .bind(version).execute(&self.pool).await.map_err(map_error)?;
            if updated.rows_affected() != 1 {
                return Err(StorageError::NotFound);
            }
            Ok(())
        })
    }
    fn check_model_planning_configuration(
        &self,
        configuration: &ModelPlanningConfiguration,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let configuration = configuration.clone();
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            check_configuration(&mut tx, &configuration).await?;
            tx.commit().await.map_err(map_error)
        })
    }
    fn create_model_planning_request(
        &self,
        owner: &UserId,
        conversation: &str,
        input: &NewModelPlanningRequest,
    ) -> BoxFuture<'_, StorageResult<ModelPlanningRequest>> {
        let ids = keys(owner, conversation, &input.request_id);
        let input = input.clone();
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let revision = replies::lock(&mut tx, ids.0, ids.1).await?;
            expire(&mut tx, ids.0, ids.1).await?;
            let existing: Option<Uuid> = sqlx::query_scalar("SELECT conversation_id FROM model_planning_requests WHERE user_id=$1 AND request_id=$2")
                .bind(ids.0).bind(ids.2).fetch_optional(&mut *tx).await.map_err(map_error)?;
            if let Some(conversation) = existing {
                if conversation != ids.1 {
                    return Err(conflict());
                }
                let stored = read(&mut tx, ids).await?;
                if stored.request.revision != input.expected_revision
                    || stored.configuration.budget.configuration_version
                        != input.configuration_version
                {
                    return Err(conflict());
                }
                tx.commit().await.map_err(map_error)?;
                return Ok(stored.request);
            }
            if revision != input.expected_revision {
                return Err(conflict());
            }
            let count: i64=sqlx::query_scalar("SELECT count(*) FROM model_planning_requests WHERE user_id=$1 AND (conversation_id=$2 OR created_at_ms>=floor(extract(epoch FROM date_trunc('day',clock_timestamp() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')*1000)::bigint)")
                .bind(ids.0).bind(ids.1).fetch_one(&mut *tx).await.map_err(map_error)?;
            if count >= 100 {
                return Err(StorageError::Conflict(
                    "model planning preview quota reached".into(),
                ));
            }
            let config: String = sqlx::query_scalar(
                "SELECT configuration::text FROM model_planning_configurations WHERE version=$1",
            )
            .bind(&input.configuration_version)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
            let configuration: ModelPlanningConfiguration =
                serde_json::from_str(&config).map_err(|_| corrupt())?;
            check_configuration(&mut tx, &configuration).await?;
            let rows=sqlx::query("SELECT id,sequence,content,floor(extract(epoch FROM created_at)*1000)::bigint AS created_ms FROM conversation_messages WHERE conversation_id=$1 ORDER BY sequence LIMIT 100")
                .bind(ids.1).fetch_all(&mut *tx).await.map_err(map_error)?;
            let snapshot = MessageSnapshot {
                revision,
                deleted: false,
                messages: rows
                    .iter()
                    .map(|r| Message {
                        id: r.get::<Uuid, _>("id").to_string(),
                        sequence: r.get("sequence"),
                        content: r.get("content"),
                        created_at_unix_ms: r.get("created_ms"),
                    })
                    .collect(),
            };
            let window = window(now(&mut tx).await?, &configuration)?;
            let quote = quote_model_planning(
                AgentRequestIdentity {
                    owner: UserId::new(ids.0.to_string()),
                    conversation: ConversationId::new(ids.1.to_string()),
                    request_id: ids.2.to_string(),
                },
                &snapshot,
                revision,
                configuration.budget.clone(),
                configuration.limits,
                window,
            )
            .map_err(|_| invalid())?;
            sqlx::query("INSERT INTO model_planning_requests(user_id,conversation_id,request_id,revision,version,configuration_version,configuration,snapshot,digest,currency,amount,status,created_at_ms,expires_at_ms) VALUES($1,$2,$3,$4,$5,$6,$7::text::jsonb,$8::text::jsonb,$9,$10,$11,'draft',$12,$13)")
                .bind(ids.0).bind(ids.1).bind(ids.2).bind(revision).bind(MODEL_PLAN_VERSION).bind(&input.configuration_version).bind(config).bind(encode(&snapshot)?).bind(quote.quote().digest()).bind(quote.quote().currency()).bind(quote.quote().amount()).bind(window.now_unix_ms).bind(window.expires_at_unix_ms).execute(&mut *tx).await.map_err(map_error)?;
            let request = read(&mut tx, ids).await?.request;
            tx.commit().await.map_err(map_error)?;
            Ok(request)
        })
    }
    fn get_model_planning_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ModelPlanningRequest>> {
        let ids = keys(owner, conversation, request);
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            replies::lock(&mut tx, ids.0, ids.1).await?;
            expire(&mut tx, ids.0, ids.1).await?;
            let request = read(&mut tx, ids).await?.request;
            tx.commit().await.map_err(map_error)?;
            Ok(request)
        })
    }
    fn list_model_planning_requests(
        &self,
        owner: &UserId,
        conversation: &str,
    ) -> BoxFuture<'_, StorageResult<Vec<ModelPlanningRequest>>> {
        let ids = keys(owner, conversation, &Uuid::nil().to_string());
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            replies::lock(&mut tx, ids.0, ids.1).await?;
            expire(&mut tx, ids.0, ids.1).await?;
            let requests: Vec<Uuid>=sqlx::query_scalar("SELECT request_id FROM model_planning_requests WHERE user_id=$1 AND conversation_id=$2 ORDER BY created_at_ms DESC,request_id DESC LIMIT 100")
                .bind(ids.0).bind(ids.1).fetch_all(&mut *tx).await.map_err(map_error)?;
            let mut result = Vec::with_capacity(requests.len());
            for request in requests {
                result.push(read(&mut tx, (ids.0, ids.1, request)).await?.request);
            }
            tx.commit().await.map_err(map_error)?;
            Ok(result)
        })
    }
    fn approve_model_planning_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        approval: &AgentQuoteApproval,
    ) -> BoxFuture<'_, StorageResult<ModelPlanningAuthorization>> {
        let ids = keys(owner, conversation, request);
        let approval = approval.clone();
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let revision = replies::lock(&mut tx, ids.0, ids.1).await?;
            expire(&mut tx, ids.0, ids.1).await?;
            let stored = read(&mut tx, ids).await?;
            let approval_json = serde_json::to_value(&approval).map_err(|_| invalid())?;
            if let Some(original) = &stored.approval {
                if *original != approval_json {
                    return Err(conflict());
                }
                tx.commit().await.map_err(map_error)?;
                return Ok(ModelPlanningAuthorization {
                    request: stored.request,
                    started: false,
                });
            }
            if stored.request.status != "draft" {
                tx.commit().await.map_err(map_error)?;
                return Err(conflict());
            }
            check_configuration(&mut tx, &stored.configuration).await?;
            let quote = frozen(ids, &stored)?;
            let time = now(&mut tx).await?;
            quote
                .quote()
                .check_approval(&approval, time, revision)
                .map_err(|_| conflict())?;
            let day: String =
                sqlx::query_scalar("SELECT (clock_timestamp() AT TIME ZONE 'UTC')::date::text")
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(map_error)?;
            reserve(&mut tx, ids, &stored, &quote, &day).await?;
            sqlx::query("UPDATE model_planning_requests SET status='queued',approval=$4,approved_at_ms=$5,budget_day=$6::text::date WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3")
                .bind(ids.0).bind(ids.1).bind(ids.2).bind(approval_json).bind(time).bind(day).execute(&mut *tx).await.map_err(map_error)?;
            let request = read(&mut tx, ids).await?.request;
            tx.commit().await.map_err(map_error)?;
            Ok(ModelPlanningAuthorization {
                request,
                started: true,
            })
        })
    }
    fn claim_model_planning_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ModelPlanningClaim>> {
        let ids = keys(owner, conversation, request);
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let revision = replies::lock(&mut tx, ids.0, ids.1).await?;
            expire(&mut tx, ids.0, ids.1).await?;
            let stored = read(&mut tx, ids).await?;
            if stored.request.status != "queued" {
                tx.commit().await.map_err(map_error)?;
                return Err(conflict());
            }
            if stored.request.revision != revision || stored.request.version != MODEL_PLAN_VERSION {
                terminate(&mut tx, ids, &stored, "stale", None, None).await?;
                tx.commit().await.map_err(map_error)?;
                return Err(conflict());
            }
            check_configuration(&mut tx, &stored.configuration).await?;
            let quote = frozen(ids, &stored)?;
            let approval: AgentQuoteApproval =
                serde_json::from_value(stored.approval.clone().ok_or_else(corrupt)?)
                    .map_err(|_| corrupt())?;
            quote
                .quote()
                .check_approval(&approval, now(&mut tx).await?, revision)
                .map_err(|_| conflict())?;
            check_receipt(&mut tx, ids, &stored).await?;
            let claim = Uuid::new_v4();
            let deadline = now(&mut tx)
                .await?
                .checked_add(60_000)
                .ok_or_else(invalid)?;
            sqlx::query("UPDATE model_planning_requests SET status='dispatching',claim_id=$4,deadline_ms=$5 WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3")
                .bind(ids.0).bind(ids.1).bind(ids.2).bind(claim).bind(deadline).execute(&mut *tx).await.map_err(map_error)?;
            let request = read(&mut tx, ids).await?.request;
            tx.commit().await.map_err(map_error)?;
            Ok(ModelPlanningClaim {
                request,
                claim_id: claim.to_string(),
                snapshot: stored.snapshot.ok_or_else(corrupt)?,
                configuration: stored.configuration,
            })
        })
    }
    fn finish_model_planning_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        claim_id: &str,
        outcome: ModelPlanningOutcome,
        usage: Option<ReplyUsage>,
    ) -> BoxFuture<'_, StorageResult<ModelPlanningRequest>> {
        let ids = keys(owner, conversation, request);
        let claim = Uuid::parse_str(claim_id).map_err(|_| invalid());
        Box::pin(async move {
            let ids = ids?;
            let claim = claim?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let revision = replies::lock(&mut tx, ids.0, ids.1).await?;
            expire(&mut tx, ids.0, ids.1).await?;
            let stored = read(&mut tx, ids).await?;
            if stored.claim_id != Some(claim) {
                return Err(conflict());
            }
            let exceeded = usage.is_some_and(|u| {
                u.input_tokens > stored.configuration.budget.input_token_bound
                    || u.output_tokens > stored.configuration.budget.output_token_bound
            });
            // 迟到的越界用量也停用配置，但不能覆盖已经结算的终态。
            if exceeded {
                sqlx::query("UPDATE model_planning_configurations SET disabled_at=COALESCE(disabled_at,clock_timestamp()) WHERE version=$1")
                    .bind(&stored.configuration.budget.configuration_version).execute(&mut *tx).await.map_err(map_error)?;
            }
            if stored.request.status == "dispatching" {
                let (status, searches) = match outcome {
                    ModelPlanningOutcome::Proposed(bytes)
                        if !exceeded && revision == stored.request.revision =>
                    {
                        match decode_model_searches(&bytes) {
                            Ok(proposal) => {
                                ("succeeded", Some(json!({"searches":proposal.searches()})))
                            }
                            Err(_) => ("failed", None),
                        }
                    }
                    ModelPlanningOutcome::Unknown => ("unknown", None),
                    _ => ("failed", None),
                };
                terminate(
                    &mut tx,
                    ids,
                    &stored,
                    status,
                    searches,
                    if status == "succeeded" || exceeded {
                        usage
                    } else {
                        None
                    },
                )
                .await?;
            }
            let request = read(&mut tx, ids).await?.request;
            tx.commit().await.map_err(map_error)?;
            Ok(request)
        })
    }
    fn cancel_model_planning_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ModelPlanningRequest>> {
        let ids = keys(owner, conversation, request);
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            replies::lock(&mut tx, ids.0, ids.1).await?;
            expire(&mut tx, ids.0, ids.1).await?;
            let stored = read(&mut tx, ids).await?;
            if matches!(
                stored.request.status.as_str(),
                "draft" | "queued" | "dispatching"
            ) {
                terminate(&mut tx, ids, &stored, "cancelled", None, None).await?;
            }
            let request = read(&mut tx, ids).await?.request;
            tx.commit().await.map_err(map_error)?;
            Ok(request)
        })
    }
}

async fn reserve(
    tx: &mut PgConnection,
    ids: Keys,
    stored: &Stored,
    quote: &ModelPlanningQuote,
    day: &str,
) -> StorageResult<()> {
    sqlx::query("INSERT INTO reply_money_daily(user_id,day,currency,occupied) VALUES($1,$2::text::date,$3,0) ON CONFLICT DO NOTHING")
        .bind(ids.0).bind(day).bind(&stored.request.currency).execute(&mut *tx).await.map_err(map_error)?;
    let amount: i64=sqlx::query_scalar("SELECT occupied FROM reply_money_daily WHERE user_id=$1 AND day=$2::text::date AND currency=$3 FOR UPDATE")
        .bind(ids.0).bind(day).bind(&stored.request.currency).fetch_one(&mut *tx).await.map_err(map_error)?;
    sqlx::query("INSERT INTO model_agent_daily(user_id,day,occupied) VALUES($1,$2::text::date,0) ON CONFLICT DO NOTHING")
        .bind(ids.0).bind(day).execute(&mut *tx).await.map_err(map_error)?;
    let models: i32 = sqlx::query_scalar(
        "SELECT occupied FROM model_agent_daily WHERE user_id=$1 AND day=$2::text::date FOR UPDATE",
    )
    .bind(ids.0)
    .bind(day)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_error)?;
    let used = quote
        .quote()
        .reserve_against(AgentBudgetUsage {
            occupied_amount: amount,
            model_calls: u32::try_from(models).map_err(|_| corrupt())?,
            tool_calls: 0,
        })
        .map_err(|_| StorageError::Conflict("model planning daily quota reached".into()))?;
    sqlx::query("UPDATE reply_money_daily SET occupied=$4 WHERE user_id=$1 AND day=$2::text::date AND currency=$3")
        .bind(ids.0).bind(day).bind(&stored.request.currency).bind(used.occupied_amount).execute(&mut *tx).await.map_err(map_error)?;
    sqlx::query("UPDATE model_agent_daily SET occupied=$3 WHERE user_id=$1 AND day=$2::text::date")
        .bind(ids.0)
        .bind(day)
        .bind(i32::try_from(used.model_calls).map_err(|_| corrupt())?)
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;
    sqlx::query("INSERT INTO reply_money_reservations(user_id,conversation_id,request_id,day,currency,model,configuration_revision,budget,reserved,request_kind) VALUES($1,$2,$3,$4::text::date,$5,$6,$7,$8::text::jsonb,$9,'model_planning')")
        .bind(ids.0).bind(ids.1).bind(ids.2).bind(day).bind(&stored.request.currency).bind(&stored.configuration.budget.model).bind(&stored.configuration.budget.configuration_version).bind(encode(&money_budget(&stored.configuration))?).bind(stored.request.amount).execute(tx).await.map_err(map_error)?;
    Ok(())
}

// 第二阶段只能从已成功持久化的建议恢复，不信任客户端传入的查询。
pub(super) async fn execution_source(
    tx: &mut PgConnection,
    ids: Keys,
) -> StorageResult<(
    ModelPlanningQuote,
    MessageSnapshot,
    Vec<personal_ai_storage::agent_plans::KnowledgeQuery>,
)> {
    let stored = read(tx, ids).await?;
    if stored.request.status != "succeeded" {
        return Err(conflict());
    }
    Ok((
        frozen(ids, &stored)?,
        stored.snapshot.ok_or_else(corrupt)?,
        stored.request.searches.ok_or_else(corrupt)?,
    ))
}
