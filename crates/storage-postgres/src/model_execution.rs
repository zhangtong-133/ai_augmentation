//! 第二阶段预算生命周期：不发送供应商请求，不保存检索证据或回答。
use crate::{PostgresStore, map_error, model_planning, replies, reply_money};
use personal_ai_agent_core::tool_execution::prepare_tool_call;
use personal_ai_agent_core::{
    budget::{CostReservation, TokenPrices},
    model_plan::{
        AgentBudgetUsage, ExecutionBudgets, MAX_QUOTE_LIFETIME_MS, ModelExecutionQuote,
        QuoteWindow, decode_model_searches, quote_model_execution, quote_model_execution_budgets,
    },
};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    model_agents::AgentQuoteApproval,
    model_execution::{
        ModelExecutionAuthorization, ModelExecutionClaim, ModelExecutionConfiguration,
        ModelExecutionOutcome, ModelExecutionRequest, ModelExecutionStore,
    },
    replies::ReplyConfiguration,
    reply_budgets::{ReplyBudget, ReplyUsage},
    tool_calls::DAILY_TOOL_CALL_LIMIT,
};
use personal_ai_tools::ToolRequest;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgConnection, Row};
use uuid::Uuid;
type Keys = (Uuid, Uuid, Uuid);
fn invalid() -> StorageError {
    StorageError::InvalidData("invalid model execution input".into())
}
fn conflict() -> StorageError {
    StorageError::Conflict("model execution unavailable or mismatched".into())
}
fn corrupt() -> StorageError {
    StorageError::Unavailable("inconsistent model execution record".into())
}
fn keys(owner: &UserId, conversation: &str, request: &str) -> StorageResult<Keys> {
    Ok((
        Uuid::parse_str(owner.as_str()).map_err(|_| invalid())?,
        Uuid::parse_str(conversation).map_err(|_| invalid())?,
        Uuid::parse_str(request).map_err(|_| invalid())?,
    ))
}
fn encode(value: &impl Serialize) -> StorageResult<String> {
    serde_json::to_string(value).map_err(|_| invalid())
}
async fn now(tx: &mut PgConnection) -> StorageResult<i64> {
    sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
        .fetch_one(tx)
        .await
        .map_err(map_error)
}
fn budgets(c: &ModelExecutionConfiguration) -> ExecutionBudgets {
    ExecutionBudgets {
        embedding: c.embedding.clone(),
        answer: c.answer.clone(),
    }
}
fn window(time: i64, c: &ModelExecutionConfiguration) -> StorageResult<QuoteWindow> {
    Ok(QuoteWindow {
        now_unix_ms: time,
        expires_at_unix_ms: time
            .checked_add(MAX_QUOTE_LIFETIME_MS)
            .ok_or_else(invalid)?
            .min(c.embedding.valid_until_unix_ms)
            .min(c.answer.valid_until_unix_ms),
    })
}
fn cost(c: &ModelExecutionConfiguration, index: usize, n: usize) -> StorageResult<CostReservation> {
    let b = if index < n { &c.embedding } else { &c.answer };
    if index < n {
        CostReservation::quote_input(
            b.input_price_per_million,
            b.input_token_bound,
            c.limits.phase_amount,
        )
    } else {
        CostReservation::quote(
            TokenPrices {
                input_per_million: b.input_price_per_million,
                output_per_million: b.output_price_per_million,
            },
            b.input_token_bound,
            b.output_token_bound,
            c.limits.phase_amount,
        )
    }
    .map_err(|_| invalid())
}
fn money(c: &ModelExecutionConfiguration, index: usize, n: usize) -> ReplyBudget {
    let b = if index < n { &c.embedding } else { &c.answer };
    ReplyBudget {
        currency: b.currency.clone(),
        provider: b.provider.clone(),
        price_version: b.price_version.clone(),
        counter_version: b.counter_version.clone(),
        input_price_per_million: b.input_price_per_million,
        output_price_per_million: b.output_price_per_million,
        input_token_bound: b.input_token_bound,
        output_token_bound: b.output_token_bound,
        request_limit: c.limits.phase_amount,
        daily_limit: c.limits.daily_amount,
    }
}
async fn check(tx: &mut PgConnection, c: &ModelExecutionConfiguration) -> StorageResult<()> {
    let row=sqlx::query("SELECT configuration::text AS config,disabled_at IS NULL AS active FROM model_execution_configurations WHERE version=$1 FOR SHARE").bind(&c.version).fetch_optional(&mut *tx).await.map_err(map_error)?.ok_or_else(conflict)?;
    let saved: ModelExecutionConfiguration =
        serde_json::from_str(row.get("config")).map_err(|_| corrupt())?;
    let time = now(tx).await?;
    if saved != *c
        || !row.get::<bool, _>("active")
        || time >= c.embedding.valid_until_unix_ms
        || time >= c.answer.valid_until_unix_ms
    {
        return Err(conflict());
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct Step {
    call_id: Uuid,
    status: String,
    claim: Option<Uuid>,
    deadline: Option<i64>,
}
#[derive(Serialize, Deserialize)]
struct Stored {
    request: ModelExecutionRequest,
    configuration: ModelExecutionConfiguration,
    approval: Option<AgentQuoteApproval>,
    day: Option<String>,
    steps: Vec<Step>,
}
async fn read(tx: &mut PgConnection, ids: Keys) -> StorageResult<Stored> {
    let data:String=sqlx::query_scalar("SELECT data::text FROM model_execution_requests WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3").bind(ids.0).bind(ids.1).bind(ids.2).fetch_one(tx).await.map_err(map_error)?;
    serde_json::from_str(&data).map_err(|_| corrupt())
}
async fn save(tx: &mut PgConnection, ids: Keys, s: &Stored) -> StorageResult<()> {
    sqlx::query("UPDATE model_execution_requests SET data=$4::text::jsonb WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3").bind(ids.0).bind(ids.1).bind(ids.2).bind(encode(s)?).execute(&mut *tx).await.map_err(map_error)?;
    for (index, step) in s.steps.iter().enumerate() {
        sqlx::query("INSERT INTO model_execution_call_audit(user_id,conversation_id,request_id,parent_request_id,ordinal,status) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(user_id,request_id) DO UPDATE SET status=EXCLUDED.status").bind(ids.0).bind(ids.1).bind(step.call_id).bind(ids.2).bind(i32::try_from(index).map_err(|_|corrupt())?).bind(&step.status).execute(&mut *tx).await.map_err(map_error)?;
    }
    Ok(())
}
async fn frozen(
    tx: &mut PgConnection,
    ids: Keys,
    s: &Stored,
) -> StorageResult<ModelExecutionQuote> {
    let (planning, _, searches) = model_planning::execution_source(tx, ids).await?;
    if s.request.searches.as_ref() != Some(&searches) {
        return Err(corrupt());
    }
    let proposal = decode_model_searches(
        &serde_json::to_vec(&json!({"searches":searches})).map_err(|_| corrupt())?,
    )
    .map_err(|_| corrupt())?;
    let quote = quote_model_execution(
        &planning,
        proposal,
        budgets(&s.configuration),
        s.configuration.limits,
        QuoteWindow {
            now_unix_ms: s.request.created_at_unix_ms,
            expires_at_unix_ms: s.request.expires_at_unix_ms,
        },
    )
    .map_err(|_| corrupt())?;
    if quote.quote().digest() != s.request.digest
        || quote.quote().amount() != s.request.amount
        || quote.quote().calls() != s.request.calls
        || quote.quote().currency() != s.request.currency
    {
        return Err(corrupt());
    }
    Ok(quote)
}
async fn verify_receipt(
    tx: &mut PgConnection,
    ids: Keys,
    s: &Stored,
    index: usize,
) -> StorageResult<()> {
    let step = &s.steps[index];
    let n = usize::try_from(s.request.calls.tool).map_err(|_| corrupt())?;
    let row=sqlx::query("SELECT budget::text AS budget,reserved,day::text AS day,currency,model,configuration_revision FROM reply_money_reservations WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3 AND request_kind='model_execution' AND charged IS NULL FOR UPDATE").bind(ids.0).bind(ids.1).bind(step.call_id).fetch_optional(&mut *tx).await.map_err(map_error)?.ok_or_else(corrupt)?;
    let saved: ReplyBudget = serde_json::from_str(row.get("budget")).map_err(|_| corrupt())?;
    let b = if index < n {
        &s.configuration.embedding
    } else {
        &s.configuration.answer
    };
    if saved != money(&s.configuration, index, n)
        || row.get::<i64, _>("reserved") != cost(&s.configuration, index, n)?.amount()
        || Some(row.get::<&str, _>("day")) != s.day.as_deref()
        || row.get::<&str, _>("currency") != s.request.currency
        || row.get::<&str, _>("model") != b.model
        || row.get::<&str, _>("configuration_revision") != b.configuration_version
    {
        return Err(corrupt());
    }
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM model_agent_daily m JOIN reply_money_daily b USING(user_id,day) JOIN tool_daily_budgets t USING(user_id,day) WHERE m.user_id=$1 AND m.day=$2::text::date AND b.currency=$3 AND b.occupied>=$4 AND m.occupied>=1 AND t.used>=$5)")
        .bind(ids.0).bind(&s.day).bind(&s.request.currency).bind(row.get::<i64,_>("reserved")).bind(i32::from(index<n)).fetch_one(tx).await.map_err(map_error)?;
    if !valid {
        return Err(corrupt());
    }
    Ok(())
}
async fn settle(
    tx: &mut PgConnection,
    ids: Keys,
    s: &Stored,
    index: usize,
    status: &str,
    usage: Option<ReplyUsage>,
    output_bytes: i32,
) -> StorageResult<()> {
    verify_receipt(tx, ids, s, index).await?;
    let step = &s.steps[index];
    let before = step.status == "reserved";
    reply_money::settle_kind(
        tx,
        (ids.0, ids.1, step.call_id),
        "model_execution",
        before,
        usage,
    )
    .await?;
    if before {
        let updated=sqlx::query("UPDATE model_agent_daily SET occupied=occupied-1 WHERE user_id=$1 AND day=$2::text::date AND occupied>=1").bind(ids.0).bind(&s.day).execute(&mut *tx).await.map_err(map_error)?;
        if updated.rows_affected() != 1 {
            return Err(corrupt());
        }
        if index < s.request.calls.tool as usize {
            let updated=sqlx::query("UPDATE tool_daily_budgets SET used=used-1 WHERE user_id=$1 AND day=$2::text::date AND used>=1").bind(ids.0).bind(&s.day).execute(&mut *tx).await.map_err(map_error)?;
            if updated.rows_affected() != 1 {
                return Err(corrupt());
            }
        }
    } else if index < s.request.calls.tool as usize {
        let tool_status = match status {
            "succeeded" => "succeeded",
            "failed" => "failed",
            _ => "timed_out",
        };
        let updated=sqlx::query("UPDATE tool_calls SET status=$3,output_bytes=$4,finished_at=clock_timestamp() WHERE user_id=$1 AND request_id=$2 AND status='running'").bind(ids.0).bind(step.call_id).bind(tool_status).bind(if status=="succeeded"{Some(output_bytes)}else{None}).execute(tx).await.map_err(map_error)?;
        if updated.rows_affected() != 1 {
            return Err(corrupt());
        }
    }
    Ok(())
}
async fn terminate(
    tx: &mut PgConnection,
    ids: Keys,
    s: &mut Stored,
    status: &str,
) -> StorageResult<()> {
    for index in 0..s.steps.len() {
        if matches!(s.steps[index].status.as_str(), "reserved" | "dispatching") {
            let final_status = if s.steps[index].status == "reserved" {
                "cancelled"
            } else {
                "unknown"
            };
            settle(tx, ids, s, index, final_status, None, 0).await?;
            s.steps[index].status = final_status.into();
        }
    }
    s.request.status = status.into();
    s.request.searches = None;
    save(tx, ids, s).await
}
async fn expire(tx: &mut PgConnection, ids: Keys, s: &mut Stored) -> StorageResult<()> {
    let time = now(tx).await?;
    if matches!(s.request.status.as_str(), "draft" | "queued" | "running")
        && (s.request.expires_at_unix_ms <= time
            || s.steps.iter().any(|step| {
                step.status == "dispatching" && step.deadline.is_some_and(|d| d <= time)
            }))
    {
        let status = if s.steps.iter().any(|step| step.status == "dispatching") {
            "unknown"
        } else {
            "expired"
        };
        terminate(tx, ids, s, status).await?;
    }
    Ok(())
}
pub(super) async fn delete_conversation(
    tx: &mut PgConnection,
    owner: Uuid,
    conversation: Uuid,
) -> StorageResult<()> {
    let requests: Vec<Uuid> = sqlx::query_scalar(
        "SELECT request_id FROM model_execution_requests WHERE user_id=$1 AND conversation_id=$2",
    )
    .bind(owner)
    .bind(conversation)
    .fetch_all(&mut *tx)
    .await
    .map_err(map_error)?;
    for request in requests {
        let ids = (owner, conversation, request);
        let mut s = read(tx, ids).await?;
        if matches!(s.request.status.as_str(), "draft" | "queued" | "running") {
            terminate(tx, ids, &mut s, "cancelled").await?;
        }
    }
    sqlx::query("DELETE FROM model_execution_requests WHERE user_id=$1 AND conversation_id=$2")
        .bind(owner)
        .bind(conversation)
        .execute(tx)
        .await
        .map_err(map_error)?;
    Ok(())
}
impl ModelExecutionStore for PostgresStore {
    fn register_model_execution_configuration(
        &self,
        c: &ModelExecutionConfiguration,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let c = c.clone();
        Box::pin(async move {
            if c.version != c.answer.configuration_version
                || c.limits.daily_tool_calls
                    > u32::try_from(DAILY_TOOL_CALL_LIMIT).map_err(|_| invalid())?
            {
                return Err(invalid());
            }
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let time = now(&mut tx).await?;
            let (embedding, answer) =
                quote_model_execution_budgets(&budgets(&c), c.limits, window(time, &c)?)
                    .map_err(|_| invalid())?;
            let total = embedding
                .amount()
                .checked_add(answer.amount())
                .ok_or_else(invalid)?;
            if total > c.limits.phase_amount
                || total > c.limits.daily_amount
                || c.limits.daily_model_calls < 2
            {
                return Err(invalid());
            }
            sqlx::query("INSERT INTO model_execution_configurations(version,configuration) VALUES($1,$2::text::jsonb) ON CONFLICT DO NOTHING").bind(&c.version).bind(encode(&c)?).execute(&mut *tx).await.map_err(map_error)?;
            check(&mut tx, &c).await?;
            tx.commit().await.map_err(map_error)
        })
    }
    fn disable_model_execution_configuration(
        &self,
        version: &str,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let version = version.to_owned();
        Box::pin(async move {
            let updated=sqlx::query("UPDATE model_execution_configurations SET disabled_at=COALESCE(disabled_at,clock_timestamp()) WHERE version=$1").bind(version).execute(&self.pool).await.map_err(map_error)?;
            if updated.rows_affected() != 1 {
                return Err(StorageError::NotFound);
            }
            Ok(())
        })
    }
    fn check_model_execution_configuration(
        &self,
        c: &ModelExecutionConfiguration,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let c = c.clone();
        Box::pin(async move {
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            check(&mut tx, &c).await?;
            tx.commit().await.map_err(map_error)
        })
    }
    fn create_model_execution_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        version: &str,
    ) -> BoxFuture<'_, StorageResult<ModelExecutionRequest>> {
        let ids = keys(owner, conversation, request);
        let version = version.to_owned();
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let revision = replies::lock(&mut tx, ids.0, ids.1).await?;
            let existing:Option<Uuid>=sqlx::query_scalar("SELECT conversation_id FROM model_execution_requests WHERE user_id=$1 AND request_id=$2").bind(ids.0).bind(ids.2).fetch_optional(&mut *tx).await.map_err(map_error)?;
            if let Some(conversation) = existing {
                if conversation != ids.1 {
                    return Err(conflict());
                }
                let mut s = read(&mut tx, ids).await?;
                if s.configuration.version != version {
                    return Err(conflict());
                }
                expire(&mut tx, ids, &mut s).await?;
                tx.commit().await.map_err(map_error)?;
                return Ok(s.request);
            }
            let (planning, snapshot, searches) =
                model_planning::execution_source(&mut tx, ids).await?;
            if snapshot.revision != revision {
                return Err(conflict());
            }
            let json: String = sqlx::query_scalar(
                "SELECT configuration::text FROM model_execution_configurations WHERE version=$1",
            )
            .bind(version)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
            let configuration: ModelExecutionConfiguration =
                serde_json::from_str(&json).map_err(|_| corrupt())?;
            check(&mut tx, &configuration).await?;
            let window = window(now(&mut tx).await?, &configuration)?;
            let proposal = decode_model_searches(
                &serde_json::to_vec(&json!({"searches":searches})).map_err(|_| invalid())?,
            )
            .map_err(|_| corrupt())?;
            let quote = quote_model_execution(
                &planning,
                proposal,
                budgets(&configuration),
                configuration.limits,
                window,
            )
            .map_err(|_| invalid())?;
            let request = ModelExecutionRequest {
                request_id: ids.2.to_string(),
                conversation_id: ids.1.to_string(),
                revision,
                digest: quote.quote().digest().into(),
                status: "draft".into(),
                currency: quote.quote().currency().into(),
                amount: quote.quote().amount(),
                calls: quote.quote().calls(),
                searches: Some(searches),
                created_at_unix_ms: window.now_unix_ms,
                expires_at_unix_ms: window.expires_at_unix_ms,
            };
            let s = Stored {
                request,
                configuration,
                approval: None,
                day: None,
                steps: vec![],
            };
            sqlx::query("INSERT INTO model_execution_requests(user_id,conversation_id,request_id,data) VALUES($1,$2,$3,$4::text::jsonb)").bind(ids.0).bind(ids.1).bind(ids.2).bind(encode(&s)?).execute(&mut *tx).await.map_err(map_error)?;
            tx.commit().await.map_err(map_error)?;
            Ok(s.request)
        })
    }
    fn get_model_execution_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ModelExecutionRequest>> {
        let ids = keys(owner, conversation, request);
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            replies::lock(&mut tx, ids.0, ids.1).await?;
            let mut s = read(&mut tx, ids).await?;
            expire(&mut tx, ids, &mut s).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(s.request)
        })
    }
    fn approve_model_execution_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        approval: &AgentQuoteApproval,
    ) -> BoxFuture<'_, StorageResult<ModelExecutionAuthorization>> {
        let ids = keys(owner, conversation, request);
        let approval = approval.clone();
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let revision = replies::lock(&mut tx, ids.0, ids.1).await?;
            let mut s = read(&mut tx, ids).await?;
            expire(&mut tx, ids, &mut s).await?;
            if let Some(original) = &s.approval {
                if original != &approval {
                    return Err(conflict());
                }
                tx.commit().await.map_err(map_error)?;
                return Ok(ModelExecutionAuthorization {
                    request: s.request,
                    started: false,
                });
            }
            if s.request.status != "draft" {
                tx.commit().await.map_err(map_error)?;
                return Err(conflict());
            }
            check(&mut tx, &s.configuration).await?;
            let quote = frozen(&mut tx, ids, &s).await?;
            quote
                .quote()
                .check_approval(&approval, now(&mut tx).await?, revision)
                .map_err(|_| conflict())?;
            reserve(&mut tx, ids, &mut s, &quote).await?;
            s.approval = Some(approval);
            s.request.status = "queued".into();
            save(&mut tx, ids, &s).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(ModelExecutionAuthorization {
                request: s.request,
                started: true,
            })
        })
    }
    fn claim_model_execution_step(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        ordinal: u32,
    ) -> BoxFuture<'_, StorageResult<ModelExecutionClaim>> {
        let ids = keys(owner, conversation, request);
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let revision = replies::lock(&mut tx, ids.0, ids.1).await?;
            let mut s = read(&mut tx, ids).await?;
            expire(&mut tx, ids, &mut s).await?;
            if !matches!(s.request.status.as_str(), "queued" | "running") {
                tx.commit().await.map_err(map_error)?;
                return Err(conflict());
            }
            if revision != s.request.revision {
                terminate(&mut tx, ids, &mut s, "stale").await?;
                tx.commit().await.map_err(map_error)?;
                return Err(conflict());
            }
            check(&mut tx, &s.configuration).await?;
            let quote = frozen(&mut tx, ids, &s).await?;
            quote
                .quote()
                .check_approval(
                    s.approval.as_ref().ok_or_else(corrupt)?,
                    now(&mut tx).await?,
                    revision,
                )
                .map_err(|_| conflict())?;
            let index = usize::try_from(ordinal).map_err(|_| invalid())?;
            let n = s.request.calls.tool as usize;
            if s.steps.len() != n + 1
                || index >= s.steps.len()
                || s.steps[index].status != "reserved"
                || s.steps[..index]
                    .iter()
                    .any(|step| step.status != "succeeded")
            {
                return Err(conflict());
            }
            verify_receipt(&mut tx, ids, &s, index).await?;
            let (_, snapshot, searches) = model_planning::execution_source(&mut tx, ids).await?;
            let claim = Uuid::new_v4();
            let call_id = s.steps[index].call_id;
            let deadline = now(&mut tx)
                .await?
                .checked_add(60_000)
                .ok_or_else(invalid)?;
            if index < n {
                let args = encode(&searches[index])?;
                let call = prepare_tool_call(
                    &call_id.to_string(),
                    "knowledge_search",
                    &ToolRequest {
                        arguments_json: args,
                    },
                )
                .map_err(|_| corrupt())?;
                sqlx::query("INSERT INTO tool_calls(user_id,request_id,tool,arguments_digest,day,input_bytes,deadline) VALUES($1,$2,'knowledge_search',$3,$4::text::date,$5,to_timestamp($6::double precision/1000))").bind(ids.0).bind(call_id).bind(&call.arguments_digest).bind(&s.day).bind(call.input_bytes).bind(deadline).execute(&mut *tx).await.map_err(map_error)?;
            }
            s.steps[index].claim = Some(claim);
            s.steps[index].deadline = Some(deadline);
            s.steps[index].status = "dispatching".into();
            s.request.status = "running".into();
            save(&mut tx, ids, &s).await?;
            let budget = if index < n {
                s.configuration.embedding.clone()
            } else {
                s.configuration.answer.clone()
            };
            let query = searches.get(index).cloned();
            tx.commit().await.map_err(map_error)?;
            Ok(ModelExecutionClaim {
                request: s.request,
                ordinal,
                claim_id: claim.to_string(),
                call_id: call_id.to_string(),
                query,
                snapshot,
                budget,
            })
        })
    }
    fn finish_model_execution_step(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        claim_id: &str,
        outcome: ModelExecutionOutcome,
        usage: Option<ReplyUsage>,
    ) -> BoxFuture<'_, StorageResult<ModelExecutionRequest>> {
        let ids = keys(owner, conversation, request);
        let claim = Uuid::parse_str(claim_id).map_err(|_| invalid());
        Box::pin(async move {
            let ids = ids?;
            let claim = claim?;
            if let ModelExecutionOutcome::Succeeded { output_bytes } = outcome
                && !(0..=65536).contains(&output_bytes)
            {
                return Err(invalid());
            }
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let revision = replies::lock(&mut tx, ids.0, ids.1).await?;
            let mut s = read(&mut tx, ids).await?;
            expire(&mut tx, ids, &mut s).await?;
            let index = s
                .steps
                .iter()
                .position(|step| step.claim == Some(claim))
                .ok_or_else(conflict)?;
            let n = s.request.calls.tool as usize;
            let b = if index < n {
                &s.configuration.embedding
            } else {
                &s.configuration.answer
            };
            let exceeded = usage.is_some_and(|u| {
                u.input_tokens > b.input_token_bound || u.output_tokens > b.output_token_bound
            });
            if exceeded {
                sqlx::query("UPDATE model_execution_configurations SET disabled_at=COALESCE(disabled_at,clock_timestamp()) WHERE version=$1").bind(&s.configuration.version).execute(&mut *tx).await.map_err(map_error)?;
            }
            if s.steps[index].status == "dispatching" {
                let (status, bytes) = match outcome {
                    ModelExecutionOutcome::Succeeded { output_bytes }
                        if !exceeded && revision == s.request.revision =>
                    {
                        ("succeeded", output_bytes)
                    }
                    ModelExecutionOutcome::Unknown => ("unknown", 0),
                    _ => ("failed", 0),
                };
                settle(
                    &mut tx,
                    ids,
                    &s,
                    index,
                    status,
                    if status == "succeeded" || exceeded {
                        usage
                    } else {
                        None
                    },
                    bytes,
                )
                .await?;
                s.steps[index].status = status.into();
                if status != "succeeded" {
                    terminate(&mut tx, ids, &mut s, status).await?;
                } else if index == n {
                    s.request.status = "succeeded".into();
                    save(&mut tx, ids, &s).await?;
                } else {
                    save(&mut tx, ids, &s).await?;
                }
            }
            tx.commit().await.map_err(map_error)?;
            Ok(s.request)
        })
    }
    fn cancel_model_execution_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ModelExecutionRequest>> {
        let ids = keys(owner, conversation, request);
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            replies::lock(&mut tx, ids.0, ids.1).await?;
            let mut s = read(&mut tx, ids).await?;
            expire(&mut tx, ids, &mut s).await?;
            if matches!(s.request.status.as_str(), "draft" | "queued" | "running") {
                terminate(&mut tx, ids, &mut s, "cancelled").await?;
            }
            tx.commit().await.map_err(map_error)?;
            Ok(s.request)
        })
    }
}

async fn reserve(
    tx: &mut PgConnection,
    ids: Keys,
    s: &mut Stored,
    quote: &ModelExecutionQuote,
) -> StorageResult<()> {
    let day: String =
        sqlx::query_scalar("SELECT (clock_timestamp() AT TIME ZONE 'UTC')::date::text")
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
    sqlx::query("INSERT INTO reply_money_daily(user_id,day,currency,occupied) VALUES($1,$2::text::date,$3,0) ON CONFLICT DO NOTHING").bind(ids.0).bind(&day).bind(&s.request.currency).execute(&mut *tx).await.map_err(map_error)?;
    sqlx::query("INSERT INTO model_agent_daily(user_id,day,occupied) VALUES($1,$2::text::date,0) ON CONFLICT DO NOTHING").bind(ids.0).bind(&day).execute(&mut *tx).await.map_err(map_error)?;
    sqlx::query("INSERT INTO tool_daily_budgets(user_id,day,used) VALUES($1,$2::text::date,0) ON CONFLICT DO NOTHING").bind(ids.0).bind(&day).execute(&mut *tx).await.map_err(map_error)?;
    let amount:i64=sqlx::query_scalar("SELECT occupied FROM reply_money_daily WHERE user_id=$1 AND day=$2::text::date AND currency=$3 FOR UPDATE").bind(ids.0).bind(&day).bind(&s.request.currency).fetch_one(&mut *tx).await.map_err(map_error)?;
    let models: i32 = sqlx::query_scalar(
        "SELECT occupied FROM model_agent_daily WHERE user_id=$1 AND day=$2::text::date FOR UPDATE",
    )
    .bind(ids.0)
    .bind(&day)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_error)?;
    let tools: i32 = sqlx::query_scalar(
        "SELECT used FROM tool_daily_budgets WHERE user_id=$1 AND day=$2::text::date FOR UPDATE",
    )
    .bind(ids.0)
    .bind(&day)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_error)?;
    let used = quote
        .quote()
        .reserve_against(AgentBudgetUsage {
            occupied_amount: amount,
            model_calls: u32::try_from(models).map_err(|_| corrupt())?,
            tool_calls: u32::try_from(tools).map_err(|_| corrupt())?,
        })
        .map_err(|_| conflict())?;
    // reserve 每笔金额复用同一日账本，按已经检查的阶段总额原子提交。
    let n = s.request.calls.tool as usize;
    for index in 0..=n {
        let call_id = Uuid::new_v4();
        let b = if index < n {
            &s.configuration.embedding
        } else {
            &s.configuration.answer
        };
        reply_money::reserve(
            tx,
            ids.0,
            (ids.1, call_id),
            &day,
            &ReplyConfiguration {
                model: b.model.clone(),
                revision: b.configuration_version.clone(),
            },
            money(&s.configuration, index, n),
            cost(&s.configuration, index, n)?,
        )
        .await?;
        sqlx::query("UPDATE reply_money_reservations SET request_kind='model_execution' WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3").bind(ids.0).bind(ids.1).bind(call_id).execute(&mut *tx).await.map_err(map_error)?;
        s.steps.push(Step {
            call_id,
            status: "reserved".into(),
            claim: None,
            deadline: None,
        });
    }
    sqlx::query("UPDATE model_agent_daily SET occupied=$3 WHERE user_id=$1 AND day=$2::text::date")
        .bind(ids.0)
        .bind(&day)
        .bind(i32::try_from(used.model_calls).map_err(|_| corrupt())?)
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;
    sqlx::query("UPDATE tool_daily_budgets SET used=$3 WHERE user_id=$1 AND day=$2::text::date")
        .bind(ids.0)
        .bind(&day)
        .bind(i32::try_from(used.tool_calls).map_err(|_| corrupt())?)
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;
    s.day = Some(day);
    Ok(())
}
