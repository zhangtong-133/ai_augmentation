use super::*;
use personal_ai_agent_core::knowledge_plan::KnowledgePlanExecutor;
use personal_ai_storage::{
    agent_plans::{AgentPlan, AgentPlanApproval, AgentPlanStore, KnowledgeQuery, NewAgentPlan},
    conversations::ConversationStore,
    messages::MessageStore,
    tool_calls::{NewToolCall, ToolCallFinish, ToolCallOutcome, ToolCallStore},
};
use personal_ai_tools::{
    BoxFuture, Tool, ToolContext, ToolError, ToolExecutor, ToolRequest, ToolResponse,
};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Fixture {
    store: Arc<PostgresStore>,
    pool: sqlx::PgPool,
    owner: UserId,
    foreign: UserId,
    conversation: String,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let store = Arc::new(PostgresStore::connect(&url).await.unwrap());
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let mut owners = Vec::new();
        for _ in 0..2 {
            let user = User {
                id: UserId::new(Uuid::new_v4().to_string()),
                email: format!("{}@agent.example", Uuid::new_v4()),
                display_name: "Agent 测试".into(),
            };
            store.save_user(&user).await.unwrap();
            owners.push(user.id);
        }
        let owner = owners.remove(0);
        let conversation = store
            .create_conversation(&owner, &Uuid::new_v4().to_string(), "研究")
            .await
            .unwrap()
            .id;
        store
            .append_message(
                &owner,
                &conversation,
                &Uuid::new_v4().to_string(),
                "用户的研究问题",
            )
            .await
            .unwrap();
        Self {
            store,
            pool,
            owner,
            foreign: owners.remove(0),
            conversation,
        }
    }
    async fn plan(&self, count: usize) -> (NewAgentPlan, AgentPlan) {
        let input = NewAgentPlan {
            request_id: Uuid::new_v4().to_string(),
            expected_revision: 1,
            searches: (0..count)
                .map(|n| KnowledgeQuery {
                    query: format!("研究查询 {n}"),
                    limit: 5,
                })
                .collect(),
        };
        let plan = self
            .store
            .create_agent_plan(&self.owner, &self.conversation, &input)
            .await
            .unwrap();
        (input, plan)
    }
    async fn approve(&self, plan: &AgentPlan) {
        assert!(
            self.store
                .approve_agent_plan(
                    &self.owner,
                    &self.conversation,
                    &plan.request_id,
                    &approval(plan)
                )
                .await
                .unwrap()
                .started
        );
    }
    async fn complete(&self, plan: &AgentPlan, call: &str, outcome: ToolCallOutcome) {
        let output = (outcome == ToolCallOutcome::Succeeded).then(|| json!({"hits":[]}));
        self.store
            .finish_tool_call(
                &self.owner,
                call,
                ToolCallFinish {
                    outcome,
                    output_bytes: output
                        .as_ref()
                        .map(|value| i32::try_from(value.to_string().len()).unwrap()),
                },
            )
            .await
            .unwrap();
        self.store
            .finish_agent_step(
                &self.owner,
                &self.conversation,
                &plan.request_id,
                call,
                output,
            )
            .await
            .unwrap();
    }
    async fn cleanup(self) {
        for owner in [self.owner, self.foreign] {
            sqlx::query("DELETE FROM users WHERE id=$1")
                .bind(Uuid::parse_str(owner.as_str()).unwrap())
                .execute(&self.pool)
                .await
                .unwrap();
        }
    }
}
fn approval(plan: &AgentPlan) -> AgentPlanApproval {
    AgentPlanApproval {
        plan_digest: plan.digest.clone(),
        accepted_call_limit: plan.tool_call_limit,
        acknowledge_embedding_cost: true,
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn agent_plan_versions_remain_visible_and_unsupported_versions_cannot_dispatch() {
    let f = Fixture::new().await;
    let (_, draft) = f.plan(1).await;
    let (_, running) = f.plan(2).await;
    f.approve(&running).await;
    sqlx::query("UPDATE agent_plans SET version='knowledge-search-future' WHERE user_id=$1")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    let reopened = PostgresStore::connect_existing(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .get_agent_plan(&f.owner, &f.conversation, &draft.request_id)
            .await
            .unwrap()
            .version,
        "knowledge-search-future"
    );
    assert!(matches!(
        reopened
            .approve_agent_plan(
                &f.owner,
                &f.conversation,
                &draft.request_id,
                &approval(&draft)
            )
            .await,
        Err(StorageError::Conflict(_))
    ));
    assert!(
        reopened
            .claim_agent_step(&f.owner, &f.conversation, &running.request_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        reopened
            .get_agent_plan(&f.owner, &f.conversation, &running.request_id)
            .await
            .unwrap()
            .status,
        "stale"
    );
    assert_eq!(
        reopened
            .audit_tool_calls(&f.owner, None)
            .await
            .unwrap()
            .used,
        0
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn agent_plan_consent_is_exact_and_concurrent_steps_spend_once_with_a_three_call_cap() {
    let f = Fixture::new().await;
    let (input, plan) = f.plan(3).await;
    assert!(
        f.store
            .claim_agent_step(&f.owner, &f.conversation, &plan.request_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.store
            .create_agent_plan(&f.owner, &f.conversation, &input)
            .await
            .unwrap(),
        plan
    );
    let mut changed = input.clone();
    changed.searches[0].query = "改变查询".into();
    assert!(matches!(
        f.store
            .create_agent_plan(&f.owner, &f.conversation, &changed)
            .await,
        Err(StorageError::Conflict(_))
    ));
    for consent in [
        AgentPlanApproval {
            plan_digest: "f".repeat(64),
            ..approval(&plan)
        },
        AgentPlanApproval {
            accepted_call_limit: 2,
            ..approval(&plan)
        },
        AgentPlanApproval {
            acknowledge_embedding_cost: false,
            ..approval(&plan)
        },
    ] {
        assert!(
            f.store
                .approve_agent_plan(&f.owner, &f.conversation, &plan.request_id, &consent)
                .await
                .is_err()
        );
    }
    assert_eq!(
        f.store.audit_tool_calls(&f.owner, None).await.unwrap().used,
        0
    );
    let consent = approval(&plan);
    let (a, b) = tokio::join!(
        f.store
            .approve_agent_plan(&f.owner, &f.conversation, &plan.request_id, &consent),
        f.store
            .approve_agent_plan(&f.owner, &f.conversation, &plan.request_id, &consent)
    );
    assert_eq!(
        usize::from(a.unwrap().started) + usize::from(b.unwrap().started),
        1
    );
    for index in 0..3 {
        let (a, b) = tokio::join!(
            f.store
                .claim_agent_step(&f.owner, &f.conversation, &plan.request_id),
            f.store
                .claim_agent_step(&f.owner, &f.conversation, &plan.request_id)
        );
        let claims: Vec<_> = [a.unwrap(), b.unwrap()].into_iter().flatten().collect();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].arguments, input.searches[index]);
        f.complete(&plan, &claims[0].call_id, ToolCallOutcome::Succeeded)
            .await;
    }
    assert!(
        f.store
            .claim_agent_step(&f.owner, &f.conversation, &plan.request_id)
            .await
            .unwrap()
            .is_none()
    );
    let finished = f
        .store
        .get_agent_plan(&f.owner, &f.conversation, &plan.request_id)
        .await
        .unwrap();
    assert_eq!(finished.status, "succeeded");
    assert_eq!(finished.attempted, 3);
    assert!(finished.steps.iter().all(|s| s.status == "succeeded"));
    assert!(
        !f.store
            .approve_agent_plan(&f.owner, &f.conversation, &plan.request_id, &consent)
            .await
            .unwrap()
            .started
    );
    let audit = f.store.audit_tool_calls(&f.owner, None).await.unwrap();
    assert_eq!(audit.used, 3);
    assert_eq!(audit.items.len(), 3);
    assert!(!serde_json::to_string(&audit).unwrap().contains("研究查询"));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn agent_plan_operations_are_owner_scoped() {
    let f = Fixture::new().await;
    let (input, plan) = f.plan(1).await;
    let consent = approval(&plan);
    for operation in [
        f.store
            .get_agent_plan(&f.foreign, &f.conversation, &plan.request_id)
            .await
            .map(|_| ()),
        f.store
            .cancel_agent_plan(&f.foreign, &f.conversation, &plan.request_id)
            .await
            .map(|_| ()),
        f.store
            .approve_agent_plan(&f.foreign, &f.conversation, &plan.request_id, &consent)
            .await
            .map(|_| ()),
        f.store
            .create_agent_plan(&f.foreign, &f.conversation, &input)
            .await
            .map(|_| ()),
    ] {
        assert!(matches!(operation, Err(StorageError::NotFound)));
    }
    assert!(matches!(
        f.store
            .claim_agent_step(&f.foreign, &f.conversation, &plan.request_id)
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store.list_agent_plans(&f.foreign, &f.conversation).await,
        Err(StorageError::NotFound)
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn agent_plan_cancellation_and_deletion_block_later_steps_and_discard_late_outputs() {
    let f = Fixture::new().await;
    let (_, plan) = f.plan(3).await;
    f.approve(&plan).await;
    let claim = f
        .store
        .claim_agent_step(&f.owner, &f.conversation, &plan.request_id)
        .await
        .unwrap()
        .unwrap();
    let cancelled = f
        .store
        .cancel_agent_plan(&f.owner, &f.conversation, &plan.request_id)
        .await
        .unwrap();
    assert_eq!(cancelled.status, "cancelled");
    f.complete(&plan, &claim.call_id, ToolCallOutcome::Succeeded)
        .await;
    assert!(
        f.store
            .claim_agent_step(&f.owner, &f.conversation, &plan.request_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.store
            .get_agent_plan(&f.owner, &f.conversation, &plan.request_id)
            .await
            .unwrap(),
        cancelled
    );
    assert_eq!(
        f.store
            .cancel_agent_plan(&f.owner, &f.conversation, &plan.request_id)
            .await
            .unwrap(),
        cancelled
    );
    let (_, second) = f.plan(2).await;
    f.approve(&second).await;
    let claim = f
        .store
        .claim_agent_step(&f.owner, &f.conversation, &second.request_id)
        .await
        .unwrap()
        .unwrap();
    f.store
        .delete_conversation(&f.owner, &f.conversation)
        .await
        .unwrap();
    f.store
        .finish_tool_call(
            &f.owner,
            &claim.call_id,
            ToolCallFinish {
                outcome: ToolCallOutcome::Succeeded,
                output_bytes: Some(11),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .finish_agent_step(
                &f.owner,
                &f.conversation,
                &second.request_id,
                &claim.call_id,
                Some(json!({"hits":[]}))
            )
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store
            .claim_agent_step(&f.owner, &f.conversation, &second.request_id)
            .await,
        Err(StorageError::NotFound)
    ));
    for table in ["agent_plans", "agent_plan_steps"] {
        let count: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE user_id=$1"))
                .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }
    assert_eq!(
        f.store.audit_tool_calls(&f.owner, None).await.unwrap().used,
        2
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn agent_plans_recheck_message_revision_before_consent_and_each_dispatch() {
    let f = Fixture::new().await;
    let (_, unapproved) = f.plan(2).await;
    let (_, running) = f.plan(2).await;
    f.approve(&running).await;
    let claim = f
        .store
        .claim_agent_step(&f.owner, &f.conversation, &running.request_id)
        .await
        .unwrap()
        .unwrap();
    f.complete(&running, &claim.call_id, ToolCallOutcome::Succeeded)
        .await;
    f.store
        .append_message(
            &f.owner,
            &f.conversation,
            &Uuid::new_v4().to_string(),
            "更新研究范围",
        )
        .await
        .unwrap();
    assert!(matches!(
        f.store
            .approve_agent_plan(
                &f.owner,
                &f.conversation,
                &unapproved.request_id,
                &approval(&unapproved)
            )
            .await,
        Err(StorageError::Conflict(_))
    ));
    assert!(
        f.store
            .claim_agent_step(&f.owner, &f.conversation, &running.request_id)
            .await
            .unwrap()
            .is_none()
    );
    for plan in [&unapproved, &running] {
        assert_eq!(
            f.store
                .get_agent_plan(&f.owner, &f.conversation, &plan.request_id)
                .await
                .unwrap()
                .status,
            "stale"
        );
    }
    assert_eq!(
        f.store.audit_tool_calls(&f.owner, None).await.unwrap().used,
        1
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn agent_plan_expiry_and_crash_recovery_never_reauthorize_or_reclaim_attempts() {
    let f = Fixture::new().await;
    let (_, expired) = f.plan(1).await;
    sqlx::query("UPDATE agent_plans SET expires_at=clock_timestamp()-interval '1 second' WHERE user_id=$1 AND request_id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&expired.request_id).unwrap()).execute(&f.pool).await.unwrap();
    assert_eq!(
        f.store
            .get_agent_plan(&f.owner, &f.conversation, &expired.request_id)
            .await
            .unwrap()
            .status,
        "expired"
    );
    assert!(
        f.store
            .approve_agent_plan(
                &f.owner,
                &f.conversation,
                &expired.request_id,
                &approval(&expired)
            )
            .await
            .is_err()
    );
    let (_, plan) = f.plan(3).await;
    f.approve(&plan).await;
    let claim = f
        .store
        .claim_agent_step(&f.owner, &f.conversation, &plan.request_id)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE tool_calls SET deadline=clock_timestamp()-interval '1 second' WHERE user_id=$1 AND request_id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&claim.call_id).unwrap()).execute(&f.pool).await.unwrap();
    let reopened = PostgresStore::connect_existing(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .get_agent_plan(&f.owner, &f.conversation, &plan.request_id)
            .await
            .unwrap()
            .status,
        "unknown"
    );
    assert!(
        !reopened
            .approve_agent_plan(
                &f.owner,
                &f.conversation,
                &plan.request_id,
                &approval(&plan)
            )
            .await
            .unwrap()
            .started
    );
    assert!(
        reopened
            .claim_agent_step(&f.owner, &f.conversation, &plan.request_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        reopened
            .audit_tool_calls(&f.owner, None)
            .await
            .unwrap()
            .used,
        1
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn agent_plan_lost_approval_response_cannot_start_a_second_executor() {
    let f = Fixture::new().await;
    let reopened = PostgresStore::connect_existing(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let (_, lost_approval) = f.plan(1).await;
    f.approve(&lost_approval).await;
    sqlx::query("UPDATE agent_plans SET run_deadline=clock_timestamp()-interval '1 second' WHERE user_id=$1 AND request_id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&lost_approval.request_id).unwrap()).execute(&f.pool).await.unwrap();
    assert_eq!(
        reopened
            .get_agent_plan(&f.owner, &f.conversation, &lost_approval.request_id)
            .await
            .unwrap()
            .status,
        "unknown"
    );
    assert!(
        !reopened
            .approve_agent_plan(
                &f.owner,
                &f.conversation,
                &lost_approval.request_id,
                &approval(&lost_approval)
            )
            .await
            .unwrap()
            .started
    );
    assert!(
        reopened
            .claim_agent_step(&f.owner, &f.conversation, &lost_approval.request_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        reopened
            .audit_tool_calls(&f.owner, None)
            .await
            .unwrap()
            .used,
        0
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn agent_plans_share_atomic_daily_tool_budget_and_stop_after_failed_attempts() {
    let f = Fixture::new().await;
    for _ in 0..99 {
        f.store
            .start_tool_call(
                &f.owner,
                &NewToolCall {
                    request_id: Uuid::new_v4().to_string(),
                    tool: "knowledge_search".into(),
                    arguments_digest: "a".repeat(64),
                    input_bytes: 2,
                },
            )
            .await
            .unwrap();
    }
    let (_, first) = f.plan(3).await;
    let (_, second) = f.plan(3).await;
    f.approve(&first).await;
    f.approve(&second).await;
    let (a, b) = tokio::join!(
        f.store
            .claim_agent_step(&f.owner, &f.conversation, &first.request_id),
        f.store
            .claim_agent_step(&f.owner, &f.conversation, &second.request_id)
    );
    let claims: Vec<_> = [(first, a.unwrap()), (second, b.unwrap())]
        .into_iter()
        .filter_map(|(plan, claim)| claim.map(|claim| (plan, claim)))
        .collect();
    assert_eq!(claims.len(), 1);
    f.complete(
        &claims[0].0,
        &claims[0].1.call_id,
        ToolCallOutcome::TimedOut,
    )
    .await;
    assert!(
        f.store
            .claim_agent_step(&f.owner, &f.conversation, &claims[0].0.request_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .list_agent_plans(&f.owner, &f.conversation)
            .await
            .unwrap()
            .iter()
            .all(|plan| plan.status == "failed")
    );
    assert_eq!(
        f.store.audit_tool_calls(&f.owner, None).await.unwrap().used,
        100
    );
    f.cleanup().await;
}

struct CountedTool(Arc<AtomicUsize>);
impl Tool for CountedTool {
    fn name(&self) -> &'static str {
        "knowledge_search"
    }
    fn description(&self) -> &'static str {
        "fixture"
    }
    fn input_schema_json(&self) -> &'static str {
        "{}"
    }
    fn execute(
        &self,
        _: &ToolContext,
        _: &ToolRequest,
    ) -> BoxFuture<'_, Result<ToolResponse, ToolError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(ToolResponse {
                content: "{\"hits\":[]}".into(),
                is_error: false,
            })
        })
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn agent_executor_never_calls_tools_when_atomic_claim_commit_fails() {
    let f = Fixture::new().await;
    let (_, plan) = f.plan(2).await;
    f.approve(&plan).await;
    let suffix = Uuid::new_v4().simple().to_string();
    let function = format!("reject_agent_{suffix}");
    let trigger = format!("reject_agent_claim_{suffix}");
    // 在 COMMIT 时失败，证明执行器必须等领取事务提交成功才调用工具。
    sqlx::query(&format!("CREATE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.user_id='{}'::uuid AND NEW.status='dispatching' THEN RAISE EXCEPTION 'fixture claim rejected'; END IF; RETURN NEW; END; $$", f.owner))
        .execute(&f.pool).await.unwrap();
    sqlx::query(&format!("CREATE CONSTRAINT TRIGGER {trigger} AFTER UPDATE ON agent_plan_steps DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {function}()"))
        .execute(&f.pool).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(ToolExecutor::new(vec![Arc::new(CountedTool(calls.clone()))]).unwrap());
    KnowledgePlanExecutor::new(f.store.clone(), executor)
        .run(&f.owner, &f.conversation, &plan.request_id)
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.store.audit_tool_calls(&f.owner, None).await.unwrap().used,
        0
    );
    assert_eq!(
        f.store
            .get_agent_plan(&f.owner, &f.conversation, &plan.request_id)
            .await
            .unwrap()
            .attempted,
        0
    );
    sqlx::query(&format!("DROP TRIGGER {trigger} ON agent_plan_steps"))
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query(&format!("DROP FUNCTION {function}()"))
        .execute(&f.pool)
        .await
        .unwrap();
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn agent_plan_storage_bounds_are_enforced_before_any_tool_budget_is_spent() {
    let f = Fixture::new().await;
    let (mut invalid, _) = f.plan(1).await;
    invalid.request_id = Uuid::new_v4().to_string();
    invalid.searches = (0..4)
        .map(|n| KnowledgeQuery {
            query: n.to_string(),
            limit: 5,
        })
        .collect();
    assert!(matches!(
        f.store
            .create_agent_plan(&f.owner, &f.conversation, &invalid)
            .await,
        Err(StorageError::InvalidData(_))
    ));
    for _ in 1..20 {
        f.plan(1).await;
    }
    invalid.searches.truncate(1);
    assert!(matches!(
        f.store
            .create_agent_plan(&f.owner, &f.conversation, &invalid)
            .await,
        Err(StorageError::Conflict(_))
    ));
    assert_eq!(
        f.store
            .list_agent_plans(&f.owner, &f.conversation)
            .await
            .unwrap()
            .len(),
        20
    );
    assert_eq!(
        f.store.audit_tool_calls(&f.owner, None).await.unwrap().used,
        0
    );
    f.cleanup().await;
}
