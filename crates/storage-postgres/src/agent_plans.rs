use crate::{PostgresStore, map_error};
use personal_ai_agent_core::{
    knowledge_plan::knowledge_plan_digest, tool_execution::prepare_tool_call,
};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    agent_plans::{
        AgentPlan, AgentPlanApproval, AgentPlanAuthorization, AgentPlanStep, AgentPlanStore,
        AgentStepClaim, NewAgentPlan, PLAN_VERSION,
    },
    tool_calls::ToolCallStart,
};
use personal_ai_tools::ToolRequest;
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

type Keys = (Uuid, Uuid, Uuid);
const FIELDS: &str = "p.request_id,p.conversation_id,p.revision,p.version,p.digest,CASE WHEN p.status='draft' AND p.expires_at<=clock_timestamp() THEN 'expired' WHEN p.status='running' AND (p.run_deadline<=clock_timestamp() OR EXISTS(SELECT 1 FROM agent_plan_steps s JOIN tool_calls t ON t.user_id=s.user_id AND t.request_id=s.call_id WHERE s.user_id=p.user_id AND s.conversation_id=p.conversation_id AND s.plan_id=p.request_id AND s.status='dispatching' AND t.deadline<=clock_timestamp())) THEN 'unknown' ELSE p.status END AS status,p.call_limit,p.attempted,floor(extract(epoch FROM p.created_at)*1000)::bigint AS created_ms,floor(extract(epoch FROM p.expires_at)*1000)::bigint AS expires_ms,floor(extract(epoch FROM p.approved_at)*1000)::bigint AS approved_ms";

fn uuid(value: &str) -> StorageResult<Uuid> {
    Uuid::parse_str(value).map_err(|_| StorageError::InvalidData("invalid agent plan id".into()))
}
fn keys(owner: &UserId, conversation: &str, request: &str) -> StorageResult<Keys> {
    Ok((uuid(owner.as_str())?, uuid(conversation)?, uuid(request)?))
}
async fn lock(tx: &mut Transaction<'_, Postgres>, ids: Keys) -> StorageResult<i64> {
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut **tx)
        .await
        .map_err(map_error)?;
    sqlx::query("SET LOCAL statement_timeout = '5s'")
        .execute(&mut **tx)
        .await
        .map_err(map_error)?;
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(ids.0)
        .fetch_one(&mut **tx)
        .await
        .map_err(map_error)?;
    sqlx::query_scalar("SELECT message_revision FROM conversations WHERE user_id=$1 AND id=$2 AND deleted_at IS NULL FOR UPDATE")
        .bind(ids.0).bind(ids.1).fetch_one(&mut **tx).await.map_err(map_error)
}
async fn read(tx: &mut Transaction<'_, Postgres>, ids: Keys) -> StorageResult<AgentPlan> {
    let row = sqlx::query(&format!("SELECT {FIELDS} FROM agent_plans p WHERE p.user_id=$1 AND p.conversation_id=$2 AND p.request_id=$3"))
        .bind(ids.0).bind(ids.1).bind(ids.2).fetch_one(&mut **tx).await.map_err(map_error)?;
    let status: String = row.get("status");
    let rows = sqlx::query("SELECT ordinal,call_id,arguments,status,output FROM agent_plan_steps WHERE user_id=$1 AND conversation_id=$2 AND plan_id=$3 ORDER BY ordinal LIMIT 3")
        .bind(ids.0).bind(ids.1).bind(ids.2).fetch_all(&mut **tx).await.map_err(map_error)?;
    let steps = rows
        .iter()
        .map(|step| {
            let mut step_status: String = step.get("status");
            if (step_status == "pending" && !matches!(status.as_str(), "draft" | "running"))
                || (step_status == "cancelled" && status != "cancelled")
            {
                step_status = "skipped".into();
            }
            if step_status == "dispatching" && status == "unknown" {
                step_status = "unknown".into();
            }
            Ok(AgentPlanStep {
                ordinal: step.get("ordinal"),
                call_id: step.get::<Uuid, _>("call_id").to_string(),
                tool: "knowledge_search".into(),
                arguments: serde_json::from_value(step.get("arguments"))
                    .map_err(|_| StorageError::InvalidData("invalid stored plan".into()))?,
                status: step_status,
                output: step.get("output"),
            })
        })
        .collect::<StorageResult<Vec<_>>>()?;
    Ok(AgentPlan {
        request_id: row.get::<Uuid, _>("request_id").to_string(),
        conversation_id: row.get::<Uuid, _>("conversation_id").to_string(),
        revision: row.get("revision"),
        version: row.get("version"),
        digest: row.get("digest"),
        status,
        tool_call_limit: row.get("call_limit"),
        attempted: row.get("attempted"),
        steps,
        created_at_unix_ms: row.get("created_ms"),
        expires_at_unix_ms: row.get("expires_ms"),
        approved_at_unix_ms: row.get("approved_ms"),
    })
}
async fn stop(tx: &mut Transaction<'_, Postgres>, ids: Keys, status: &str) -> StorageResult<()> {
    sqlx::query("UPDATE agent_plans SET status=$4 WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3")
        .bind(ids.0).bind(ids.1).bind(ids.2).bind(status).execute(&mut **tx).await.map_err(map_error)?;
    sqlx::query("UPDATE agent_plan_steps SET status='cancelled' WHERE user_id=$1 AND conversation_id=$2 AND plan_id=$3 AND status='pending'")
        .bind(ids.0).bind(ids.1).bind(ids.2).execute(&mut **tx).await.map_err(map_error)?;
    Ok(())
}

impl AgentPlanStore for PostgresStore {
    fn create_agent_plan(
        &self,
        owner: &UserId,
        conversation: &str,
        input: &NewAgentPlan,
    ) -> BoxFuture<'_, StorageResult<AgentPlan>> {
        let ids = keys(owner, conversation, &input.request_id);
        let mut input = input.clone();
        Box::pin(async move {
            input.validate()?;
            let ids = ids?;
            input.request_id = ids.2.to_string();
            let digest =
                knowledge_plan_digest(&UserId::new(ids.0.to_string()), &ids.1.to_string(), &input);
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let revision = lock(&mut tx, ids).await?;
            match read(&mut tx, ids).await {
                Ok(plan) => {
                    if plan.digest != digest {
                        return Err(StorageError::Conflict("plan request already used".into()));
                    }
                    tx.commit().await.map_err(map_error)?;
                    return Ok(plan);
                }
                Err(StorageError::NotFound) => {}
                Err(error) => return Err(error),
            }
            if revision != input.expected_revision {
                return Err(StorageError::Conflict("plan revision changed".into()));
            }
            let row = sqlx::query("SELECT count(*) AS total,count(*) FILTER (WHERE conversation_id=$2) AS in_conversation FROM agent_plans WHERE user_id=$1")
                .bind(ids.0).bind(ids.1).fetch_one(&mut *tx).await.map_err(map_error)?;
            if row.get::<i64, _>("total") >= 100 || row.get::<i64, _>("in_conversation") >= 20 {
                return Err(StorageError::Conflict("plan quota reached".into()));
            }
            let limit = i32::try_from(input.searches.len())
                .map_err(|_| StorageError::InvalidData("invalid plan limit".into()))?;
            sqlx::query("INSERT INTO agent_plans(user_id,conversation_id,request_id,revision,digest,call_limit,version) VALUES($1,$2,$3,$4,$5,$6,$7)")
                .bind(ids.0).bind(ids.1).bind(ids.2).bind(revision).bind(digest).bind(limit).bind(PLAN_VERSION).execute(&mut *tx).await.map_err(map_error)?;
            for (index, arguments) in input.searches.iter().enumerate() {
                let ordinal = i32::try_from(index + 1)
                    .map_err(|_| StorageError::InvalidData("invalid plan ordinal".into()))?;
                sqlx::query("INSERT INTO agent_plan_steps(user_id,conversation_id,plan_id,ordinal,call_id,arguments) VALUES($1,$2,$3,$4,$5,$6)")
                    .bind(ids.0).bind(ids.1).bind(ids.2).bind(ordinal).bind(Uuid::new_v4()).bind(json!(arguments)).execute(&mut *tx).await.map_err(map_error)?;
            }
            let plan = read(&mut tx, ids).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(plan)
        })
    }

    fn get_agent_plan(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<AgentPlan>> {
        let ids = keys(owner, conversation, request);
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            sqlx::query("SELECT id FROM conversations WHERE user_id=$1 AND id=$2 AND deleted_at IS NULL FOR SHARE")
                .bind(ids.0).bind(ids.1).fetch_one(&mut *tx).await.map_err(map_error)?;
            let plan = read(&mut tx, ids).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(plan)
        })
    }

    fn list_agent_plans(
        &self,
        owner: &UserId,
        conversation: &str,
    ) -> BoxFuture<'_, StorageResult<Vec<AgentPlan>>> {
        let ids = keys(owner, conversation, &Uuid::nil().to_string());
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            sqlx::query("SELECT id FROM conversations WHERE user_id=$1 AND id=$2 AND deleted_at IS NULL FOR SHARE")
                .bind(ids.0).bind(ids.1).fetch_one(&mut *tx).await.map_err(map_error)?;
            let requests: Vec<Uuid> = sqlx::query_scalar("SELECT request_id FROM agent_plans WHERE user_id=$1 AND conversation_id=$2 ORDER BY created_at,request_id LIMIT 20")
                .bind(ids.0).bind(ids.1).fetch_all(&mut *tx).await.map_err(map_error)?;
            let mut plans = Vec::new();
            for request in requests {
                plans.push(read(&mut tx, (ids.0, ids.1, request)).await?);
            }
            tx.commit().await.map_err(map_error)?;
            Ok(plans)
        })
    }

    fn approve_agent_plan(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        approval: &AgentPlanApproval,
    ) -> BoxFuture<'_, StorageResult<AgentPlanAuthorization>> {
        let ids = keys(owner, conversation, request);
        let approval = approval.clone();
        Box::pin(async move {
            if !approval.acknowledge_embedding_cost
                || !(1..=3).contains(&approval.accepted_call_limit)
            {
                return Err(StorageError::InvalidData("plan consent required".into()));
            }
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let revision = lock(&mut tx, ids).await?;
            let plan = read(&mut tx, ids).await?;
            if plan.version != PLAN_VERSION {
                return Err(StorageError::Conflict("plan version unsupported".into()));
            }
            if plan.digest != approval.plan_digest
                || plan.tool_call_limit != approval.accepted_call_limit
            {
                return Err(StorageError::Conflict("plan approval mismatch".into()));
            }
            if plan.approved_at_unix_ms.is_some() {
                tx.commit().await.map_err(map_error)?;
                return Ok(AgentPlanAuthorization {
                    plan,
                    started: false,
                });
            }
            if plan.status != "draft" {
                return Err(StorageError::Conflict("plan no longer approvable".into()));
            }
            if revision != plan.revision {
                stop(&mut tx, ids, "stale").await?;
                tx.commit().await.map_err(map_error)?;
                return Err(StorageError::Conflict("plan revision changed".into()));
            }
            sqlx::query("UPDATE agent_plans SET status='running',approved_at=clock_timestamp(),run_deadline=clock_timestamp()+interval '150 seconds' WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3")
                .bind(ids.0).bind(ids.1).bind(ids.2).execute(&mut *tx).await.map_err(map_error)?;
            let plan = read(&mut tx, ids).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(AgentPlanAuthorization {
                plan,
                started: true,
            })
        })
    }

    fn cancel_agent_plan(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<AgentPlan>> {
        let ids = keys(owner, conversation, request);
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            lock(&mut tx, ids).await?;
            let plan = read(&mut tx, ids).await?;
            if matches!(
                plan.status.as_str(),
                "draft" | "running" | "unknown" | "expired"
            ) {
                stop(&mut tx, ids, "cancelled").await?;
                sqlx::query("UPDATE agent_plan_steps SET output=NULL,status='cancelled' WHERE user_id=$1 AND conversation_id=$2 AND plan_id=$3")
                    .bind(ids.0).bind(ids.1).bind(ids.2).execute(&mut *tx).await.map_err(map_error)?;
            }
            let plan = read(&mut tx, ids).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(plan)
        })
    }

    fn claim_agent_step(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Option<AgentStepClaim>>> {
        let ids = keys(owner, conversation, request);
        Box::pin(async move {
            let ids = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            let revision = lock(&mut tx, ids).await?;
            let plan = read(&mut tx, ids).await?;
            if plan.status != "running"
                || plan.steps.iter().any(|step| step.status == "dispatching")
            {
                tx.commit().await.map_err(map_error)?;
                return Ok(None);
            }
            if revision != plan.revision || plan.version != PLAN_VERSION {
                stop(&mut tx, ids, "stale").await?;
                tx.commit().await.map_err(map_error)?;
                return Ok(None);
            }
            let step = plan
                .steps
                .iter()
                .find(|step| step.status == "pending")
                .ok_or_else(|| StorageError::Conflict("plan has no pending step".into()))?;
            if plan.attempted >= plan.tool_call_limit {
                return Err(StorageError::Conflict("plan call limit reached".into()));
            }
            let call_id = uuid(&step.call_id)?;
            let call = prepare_tool_call(
                &step.call_id,
                "knowledge_search",
                &ToolRequest {
                    arguments_json: json!(step.arguments).to_string(),
                },
            )
            .map_err(|_| StorageError::InvalidData("invalid stored plan".into()))?;
            match crate::tool_calls::start_locked(&mut tx, ids.0, call_id, &call).await {
                Ok(ToolCallStart::Started(_)) => {}
                Ok(ToolCallStart::Existing(_)) => {
                    // 一次性 ID 已占用时绝不再次执行，保留结果未知状态。
                    stop(&mut tx, ids, "unknown").await?;
                    tx.commit().await.map_err(map_error)?;
                    return Ok(None);
                }
                Err(StorageError::Conflict(reason)) if reason == "tool call quota reached" => {
                    stop(&mut tx, ids, "failed").await?;
                    tx.commit().await.map_err(map_error)?;
                    return Ok(None);
                }
                Err(error) => return Err(error),
            }
            sqlx::query("UPDATE agent_plan_steps SET status='dispatching' WHERE user_id=$1 AND conversation_id=$2 AND plan_id=$3 AND call_id=$4 AND status='pending'")
                .bind(ids.0).bind(ids.1).bind(ids.2).bind(call_id).execute(&mut *tx).await.map_err(map_error)?;
            sqlx::query("UPDATE agent_plans SET attempted=attempted+1 WHERE user_id=$1 AND conversation_id=$2 AND request_id=$3")
                .bind(ids.0).bind(ids.1).bind(ids.2).execute(&mut *tx).await.map_err(map_error)?;
            let claim = AgentStepClaim {
                call_id: step.call_id.clone(),
                arguments: step.arguments.clone(),
            };
            tx.commit().await.map_err(map_error)?;
            Ok(Some(claim))
        })
    }

    fn finish_agent_step(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        call: &str,
        output: Option<Value>,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let ids = keys(owner, conversation, request).and_then(|ids| Ok((ids, uuid(call)?)));
        Box::pin(async move {
            if output
                .as_ref()
                .is_some_and(|value| !value.is_object() || value.to_string().len() > 65536)
            {
                return Err(StorageError::InvalidData("invalid plan output".into()));
            }
            let (ids, call) = ids?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            lock(&mut tx, ids).await?;
            let plan = read(&mut tx, ids).await?;
            if plan.status != "running" {
                tx.commit().await.map_err(map_error)?;
                return Ok(());
            }
            let step = plan
                .steps
                .iter()
                .find(|step| step.call_id == call.to_string())
                .ok_or(StorageError::NotFound)?;
            if step.status != "dispatching" {
                if step.output == output {
                    return Ok(());
                }
                return Err(StorageError::Conflict("plan step already finished".into()));
            }
            let audit = sqlx::query("SELECT status,deadline>clock_timestamp() AS timely FROM tool_calls WHERE user_id=$1 AND request_id=$2")
                .bind(ids.0).bind(call).fetch_one(&mut *tx).await.map_err(map_error)?;
            let status: String = audit.get("status");
            if !audit.get::<bool, _>("timely") || status == "running" {
                stop(&mut tx, ids, "unknown").await?;
            } else {
                if (status == "succeeded") != output.is_some() {
                    return Err(StorageError::Conflict("tool audit outcome mismatch".into()));
                }
                let succeeded = output.is_some();
                sqlx::query("UPDATE agent_plan_steps SET status=$5,output=$6 WHERE user_id=$1 AND conversation_id=$2 AND plan_id=$3 AND call_id=$4")
                    .bind(ids.0).bind(ids.1).bind(ids.2).bind(call).bind(if succeeded { "succeeded" } else { "failed" }).bind(output).execute(&mut *tx).await.map_err(map_error)?;
                if !succeeded {
                    stop(&mut tx, ids, "failed").await?;
                } else if plan.attempted == plan.tool_call_limit {
                    stop(&mut tx, ids, "succeeded").await?;
                }
            }
            tx.commit().await.map_err(map_error)?;
            Ok(())
        })
    }
}
