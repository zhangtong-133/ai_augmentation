use super::*;
use personal_ai_storage::{
    conversations::ConversationStore,
    messages::MessageStore,
    model_agents::{
        AgentBudgetLimits, AgentQuoteApproval, ModelCallBudget, ModelPlanningClaim,
        ModelPlanningConfiguration, ModelPlanningOutcome, ModelPlanningRequest, ModelPlanningStore,
        NewModelPlanningRequest,
    },
    replies::{ReplyConfiguration, ReplyContext, ReplyStore},
    reply_budgets::{BudgetedReplyStore, ReplyBudget, ReplyBudgetPlanner, ReplyUsage},
    reply_operations::ReplyOperationsStore,
};
use std::sync::Arc;

fn id() -> String {
    Uuid::new_v4().to_string()
}
const AMOUNT: i64 = 2148;
fn proposal() -> ModelPlanningOutcome {
    ModelPlanningOutcome::Proposed(br#"{"searches":[{"query":"research","limit":5}]}"#.to_vec())
}
fn approval(request: &ModelPlanningRequest) -> AgentQuoteApproval {
    AgentQuoteApproval {
        digest: request.digest.clone(),
        accepted_currency: request.currency.clone(),
        accepted_amount: request.amount,
        accepted_calls: request.calls,
        acknowledge_cost: true,
    }
}
struct Fixture {
    store: Arc<PostgresStore>,
    pool: sqlx::PgPool,
    owner: UserId,
    foreign: UserId,
    conversation: String,
    configuration: ModelPlanningConfiguration,
}
impl Fixture {
    async fn new(amount: i64, calls: u32) -> Self {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let store = Arc::new(PostgresStore::connect(&url).await.unwrap());
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let mut users = Vec::new();
        for _ in 0..2 {
            let user = User {
                id: UserId::new(id()),
                email: format!("{}@model-agent.example", id()),
                display_name: "模型规划测试".into(),
            };
            store.save_user(&user).await.unwrap();
            users.push(user.id);
        }
        let owner = users.remove(0);
        let conversation = store
            .create_conversation(&owner, &id(), "规划")
            .await
            .unwrap()
            .id;
        store
            .append_message(&owner, &conversation, &id(), "用户的私有研究问题")
            .await
            .unwrap();
        let time: i64 =
            sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
                .fetch_one(&pool)
                .await
                .unwrap();
        let configuration = ModelPlanningConfiguration {
            budget: ModelCallBudget {
                configuration_version: id(),
                provider: "fixture".into(),
                model: "fixed-fixture-v1".into(),
                price_version: "price-v1".into(),
                counter_version: "counter-v1".into(),
                currency: "USD".into(),
                input_price_per_million: 1_000_000,
                output_price_per_million: 1_000_000,
                input_token_bound: 100,
                output_token_bound: 2048,
                valid_until_unix_ms: time + 3_600_000,
            },
            limits: AgentBudgetLimits {
                phase_amount: AMOUNT,
                daily_amount: amount,
                daily_model_calls: calls,
                daily_tool_calls: 100,
            },
        };
        store
            .register_model_planning_configuration(&configuration)
            .await
            .unwrap();
        Self {
            store,
            pool,
            owner,
            foreign: users.remove(0),
            conversation,
            configuration,
        }
    }
    async fn draft(&self) -> ModelPlanningRequest {
        self.store
            .create_model_planning_request(
                &self.owner,
                &self.conversation,
                &NewModelPlanningRequest {
                    request_id: id(),
                    expected_revision: 1,
                    configuration_version: self.configuration.budget.configuration_version.clone(),
                },
            )
            .await
            .unwrap()
    }
    async fn approve(&self, request: &ModelPlanningRequest) {
        assert!(
            self.store
                .approve_model_planning_request(
                    &self.owner,
                    &self.conversation,
                    &request.request_id,
                    &approval(request)
                )
                .await
                .unwrap()
                .started
        );
    }
    async fn claim(&self, request: &ModelPlanningRequest) -> ModelPlanningClaim {
        self.store
            .claim_model_planning_request(&self.owner, &self.conversation, &request.request_id)
            .await
            .unwrap()
    }
    async fn get(&self, request: &ModelPlanningRequest) -> ModelPlanningRequest {
        self.store
            .get_model_planning_request(&self.owner, &self.conversation, &request.request_id)
            .await
            .unwrap()
    }
    async fn money(&self) -> i64 {
        sqlx::query_scalar(
            "SELECT COALESCE(sum(occupied),0)::bigint FROM reply_money_daily WHERE user_id=$1",
        )
        .bind(Uuid::parse_str(self.owner.as_str()).unwrap())
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
    async fn calls(&self) -> i64 {
        sqlx::query_scalar(
            "SELECT COALESCE(sum(occupied),0)::bigint FROM model_agent_daily WHERE user_id=$1",
        )
        .bind(Uuid::parse_str(self.owner.as_str()).unwrap())
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
    async fn finish(
        &self,
        claim: &ModelPlanningClaim,
        outcome: ModelPlanningOutcome,
        usage: Option<ReplyUsage>,
    ) -> ModelPlanningRequest {
        self.store
            .finish_model_planning_request(
                &self.owner,
                &self.conversation,
                &claim.request.request_id,
                &claim.claim_id,
                outcome,
                usage,
            )
            .await
            .unwrap()
    }
    async fn audit(&self) -> personal_ai_storage::reply_operations::ReplyMoneyAudit {
        let day: String =
            sqlx::query_scalar("SELECT (clock_timestamp() AT TIME ZONE 'UTC')::date::text")
                .fetch_one(&self.pool)
                .await
                .unwrap();
        self.store
            .audit_reply_money(&self.owner, &day, "USD", None)
            .await
            .unwrap()
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_planning_preview_consent_claim_and_result_are_durable_and_private() {
    let f = Fixture::new(AMOUNT * 10, 10).await;
    let input = NewModelPlanningRequest {
        request_id: id(),
        expected_revision: 1,
        configuration_version: f.configuration.budget.configuration_version.clone(),
    };
    let (a, b) = tokio::join!(
        f.store
            .create_model_planning_request(&f.owner, &f.conversation, &input),
        f.store
            .create_model_planning_request(&f.owner, &f.conversation, &input)
    );
    let request = a.unwrap();
    assert_eq!(request, b.unwrap());
    assert_eq!((f.money().await, f.calls().await), (0, 0));
    let consent = approval(&request);
    let (a, b) = tokio::join!(
        f.store.approve_model_planning_request(
            &f.owner,
            &f.conversation,
            &request.request_id,
            &consent
        ),
        f.store.approve_model_planning_request(
            &f.owner,
            &f.conversation,
            &request.request_id,
            &consent
        )
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_ne!(a.started, b.started);
    assert_eq!(a.request, b.request);
    assert_eq!((f.money().await, f.calls().await), (AMOUNT, 1));
    let (a, b) = tokio::join!(
        f.store
            .claim_model_planning_request(&f.owner, &f.conversation, &request.request_id),
        f.store
            .claim_model_planning_request(&f.owner, &f.conversation, &request.request_id)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let claim = a.or(b).unwrap();
    assert_eq!(claim.snapshot.messages[0].content, "用户的私有研究问题");
    let result = f
        .finish(
            &claim,
            proposal(),
            Some(ReplyUsage {
                input_tokens: 10,
                output_tokens: 20,
            }),
        )
        .await;
    assert_eq!(result.status, "succeeded");
    assert_eq!(result.searches.as_ref().unwrap()[0].query, "research");
    assert_eq!(f.money().await, 30);
    assert_eq!(f.calls().await, 1);
    assert_eq!(
        f.finish(
            &claim,
            proposal(),
            Some(ReplyUsage {
                input_tokens: 1,
                output_tokens: 1
            })
        )
        .await,
        result
    );
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .create_model_planning_request(&f.owner, &f.conversation, &input)
            .await
            .unwrap(),
        result
    );
    assert!(
        !reopened
            .approve_model_planning_request(
                &f.owner,
                &f.conversation,
                &request.request_id,
                &consent
            )
            .await
            .unwrap()
            .started
    );
    assert!(
        reopened
            .claim_model_planning_request(&f.owner, &f.conversation, &request.request_id)
            .await
            .is_err()
    );
    assert!(f.audit().await.consistent);
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_planning_freezes_maximum_escaped_history_without_exposing_omitted_content() {
    let f = Fixture::new(AMOUNT * 10, 10).await;
    let text = "\u{1}".repeat(4096);
    sqlx::query("UPDATE conversation_messages SET content=$2 WHERE conversation_id=$1")
        .bind(Uuid::parse_str(&f.conversation).unwrap())
        .bind(&text)
        .execute(&f.pool)
        .await
        .unwrap();
    for _ in 1..100 {
        f.store
            .append_message(&f.owner, &f.conversation, &id(), &text)
            .await
            .unwrap();
    }
    let request = f
        .store
        .create_model_planning_request(
            &f.owner,
            &f.conversation,
            &NewModelPlanningRequest {
                request_id: id(),
                expected_revision: 100,
                configuration_version: f.configuration.budget.configuration_version.clone(),
            },
        )
        .await
        .unwrap();
    f.approve(&request).await;
    let claim = f.claim(&request).await;
    assert_eq!(claim.snapshot.messages.len(), 100);
    assert_eq!(claim.configuration, f.configuration);
    assert!(!serde_json::to_string(&request).unwrap().contains("\\u0001"));
    let result = f.finish(&claim, proposal(), None).await;
    assert_eq!(result.status, "succeeded");
    assert_eq!(f.money().await, AMOUNT);
    assert_eq!(f.calls().await, 1);
    let audit = f.audit().await;
    assert!(audit.consistent);
    assert_eq!(audit.items[0].request_kind, "model_planning");
    assert_eq!(audit.items[0].reply_status.as_deref(), Some("succeeded"));
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_planning_consent_is_exact_and_metadata_is_owner_scoped() {
    let f = Fixture::new(AMOUNT * 10, 10).await;
    let request = f.draft().await;
    let serialized = serde_json::to_string(&request).unwrap();
    assert!(!serialized.contains("用户的私有研究问题"));
    assert!(!serialized.contains("snapshot"));
    assert!(matches!(
        f.store
            .get_model_planning_request(&f.foreign, &f.conversation, &request.request_id)
            .await,
        Err(StorageError::NotFound)
    ));
    let consent = approval(&request);
    assert!(matches!(
        f.store
            .approve_model_planning_request(
                &f.foreign,
                &f.conversation,
                &request.request_id,
                &consent
            )
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store
            .claim_model_planning_request(&f.foreign, &f.conversation, &request.request_id)
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store
            .cancel_model_planning_request(&f.foreign, &f.conversation, &request.request_id)
            .await,
        Err(StorageError::NotFound)
    ));
    for field in 0..4 {
        let mut approval = approval(&request);
        match field {
            0 => approval.digest = "wrong".into(),
            1 => approval.accepted_amount -= 1,
            2 => approval.accepted_calls.embedding = 1,
            _ => approval.acknowledge_cost = false,
        }
        assert!(
            f.store
                .approve_model_planning_request(
                    &f.owner,
                    &f.conversation,
                    &request.request_id,
                    &approval
                )
                .await
                .is_err()
        );
        assert_eq!(f.money().await, 0);
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_planning_late_exceeded_usage_disables_configuration_without_rewriting_settlement() {
    let f = Fixture::new(AMOUNT * 10, 10).await;
    let request = f.draft().await;
    f.approve(&request).await;
    let claim = f.claim(&request).await;
    f.store
        .cancel_model_planning_request(&f.owner, &f.conversation, &request.request_id)
        .await
        .unwrap();
    let result = f
        .finish(
            &claim,
            proposal(),
            Some(ReplyUsage {
                input_tokens: 101,
                output_tokens: 1,
            }),
        )
        .await;
    assert_eq!(result.status, "cancelled");
    assert!(result.searches.is_none());
    assert!(
        f.store
            .check_model_planning_configuration(&f.configuration)
            .await
            .is_err()
    );
    assert_eq!(f.money().await, AMOUNT);
    assert_eq!(f.calls().await, 1);
    let audit = f.audit().await;
    assert!(audit.consistent);
    assert_eq!(audit.counts.retained, 1);
    assert_eq!(audit.counts.usage_exceeded, 0);
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_planning_last_money_and_call_slots_are_atomic_and_shared_across_configs() {
    for (amount, calls) in [(AMOUNT, 10), (AMOUNT * 10, 1)] {
        let f = Fixture::new(amount, calls).await;
        let first = f.draft().await;
        let mut next = f.configuration.clone();
        next.budget.configuration_version = id();
        f.store
            .register_model_planning_configuration(&next)
            .await
            .unwrap();
        let second = f
            .store
            .create_model_planning_request(
                &f.owner,
                &f.conversation,
                &NewModelPlanningRequest {
                    request_id: id(),
                    expected_revision: 1,
                    configuration_version: next.budget.configuration_version,
                },
            )
            .await
            .unwrap();
        let c1 = approval(&first);
        let c2 = approval(&second);
        let (a, b) = tokio::join!(
            f.store.approve_model_planning_request(
                &f.owner,
                &f.conversation,
                &first.request_id,
                &c1
            ),
            f.store.approve_model_planning_request(
                &f.owner,
                &f.conversation,
                &second.request_id,
                &c2
            )
        );
        assert_ne!(a.is_ok(), b.is_ok());
        assert_eq!(f.money().await, AMOUNT);
        assert_eq!(f.calls().await, 1);
        let winner = a.or(b).unwrap().request;
        f.store
            .cancel_model_planning_request(&f.owner, &f.conversation, &winner.request_id)
            .await
            .unwrap();
        assert_eq!(f.money().await, 0);
        assert_eq!(f.calls().await, 0);
        assert!(f.audit().await.consistent);
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_planning_configuration_is_immutable_disabled_and_rechecked_at_dispatch() {
    let f = Fixture::new(AMOUNT * 10, 10).await;
    f.store
        .register_model_planning_configuration(&f.configuration)
        .await
        .unwrap();
    let mut changed = f.configuration.clone();
    changed.budget.input_price_per_million += 1;
    assert!(
        f.store
            .register_model_planning_configuration(&changed)
            .await
            .is_err()
    );
    let request = f.draft().await;
    f.approve(&request).await;
    f.store
        .disable_model_planning_configuration(&f.configuration.budget.configuration_version)
        .await
        .unwrap();
    f.store
        .disable_model_planning_configuration(&f.configuration.budget.configuration_version)
        .await
        .unwrap();
    assert!(
        f.store
            .register_model_planning_configuration(&f.configuration)
            .await
            .is_err()
    );
    assert!(
        f.store
            .claim_model_planning_request(&f.owner, &f.conversation, &request.request_id)
            .await
            .is_err()
    );
    assert!(
        f.store
            .check_model_planning_configuration(&f.configuration)
            .await
            .is_err()
    );
    assert_eq!(f.get(&request).await.status, "queued");
    f.store
        .cancel_model_planning_request(&f.owner, &f.conversation, &request.request_id)
        .await
        .unwrap();
    assert_eq!(f.money().await, 0);
    assert_eq!(f.calls().await, 0);
    let mut invalid = f.configuration.clone();
    invalid.budget.configuration_version = id();
    invalid.budget.output_token_bound = 1024;
    assert!(
        f.store
            .register_model_planning_configuration(&invalid)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_planning_invalid_unknown_and_exceeded_results_keep_attempts_and_stop_config() {
    let f = Fixture::new(AMOUNT * 10, 10).await;
    for outcome in [
        ModelPlanningOutcome::Proposed(
            br#"{"searches":[{"query":"q","limit":1,"tool":"shell"}]}"#.to_vec(),
        ),
        ModelPlanningOutcome::Unknown,
        ModelPlanningOutcome::Failed,
    ] {
        let request = f.draft().await;
        f.approve(&request).await;
        let claim = f.claim(&request).await;
        let result = f
            .finish(
                &claim,
                outcome,
                Some(ReplyUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                }),
            )
            .await;
        assert!(matches!(result.status.as_str(), "failed" | "unknown"));
        assert!(result.searches.is_none());
    }
    assert_eq!(f.money().await, AMOUNT * 3);
    assert_eq!(f.calls().await, 3);
    let request = f.draft().await;
    f.approve(&request).await;
    let claim = f.claim(&request).await;
    let result = f
        .finish(
            &claim,
            proposal(),
            Some(ReplyUsage {
                input_tokens: 101,
                output_tokens: 1,
            }),
        )
        .await;
    assert_eq!(result.status, "failed");
    assert!(
        f.store
            .check_model_planning_configuration(&f.configuration)
            .await
            .is_err()
    );
    let audit = f.audit().await;
    assert!(audit.consistent);
    assert_eq!(audit.counts.usage_exceeded, 1);
    assert_eq!(f.calls().await, 4);
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_planning_cancel_claim_races_never_refund_dispatched_calls_or_save_late_output() {
    let f = Fixture::new(AMOUNT * 10, 10).await;
    let request = f.draft().await;
    f.approve(&request).await;
    let (claim, cancel) = tokio::join!(
        f.store
            .claim_model_planning_request(&f.owner, &f.conversation, &request.request_id),
        f.store
            .cancel_model_planning_request(&f.owner, &f.conversation, &request.request_id)
    );
    assert_eq!(cancel.unwrap().status, "cancelled");
    if let Ok(claim) = claim {
        assert_eq!(f.money().await, AMOUNT);
        assert_eq!(f.calls().await, 1);
        let late = f
            .finish(
                &claim,
                proposal(),
                Some(ReplyUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                }),
            )
            .await;
        assert_eq!(late.status, "cancelled");
        assert!(late.searches.is_none());
        assert_eq!(f.money().await, AMOUNT);
        f.finish(
            &claim,
            proposal(),
            Some(ReplyUsage {
                input_tokens: 101,
                output_tokens: 1,
            }),
        )
        .await;
        assert!(
            f.store
                .check_model_planning_configuration(&f.configuration)
                .await
                .is_err()
        );
    } else {
        assert_eq!(f.money().await, 0);
        assert_eq!(f.calls().await, 0);
    }
    assert!(f.audit().await.consistent);
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_planning_expiry_refunds_only_undispatched_requests_and_never_reclaims() {
    let f = Fixture::new(AMOUNT * 10, 10).await;
    let draft = f.draft().await;
    sqlx::query(
        "UPDATE model_planning_requests SET created_at_ms=0,expires_at_ms=1 WHERE request_id=$1",
    )
    .bind(Uuid::parse_str(&draft.request_id).unwrap())
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(f.get(&draft).await.status, "expired");
    assert_eq!(f.money().await, 0);
    let queued = f.draft().await;
    f.approve(&queued).await;
    sqlx::query(
        "UPDATE model_planning_requests SET created_at_ms=0,expires_at_ms=1 WHERE request_id=$1",
    )
    .bind(Uuid::parse_str(&queued.request_id).unwrap())
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(f.get(&queued).await.status, "expired");
    assert_eq!(f.money().await, 0);
    assert_eq!(f.calls().await, 0);
    let request = f.draft().await;
    f.approve(&request).await;
    let claim = f.claim(&request).await;
    sqlx::query("UPDATE model_planning_requests SET deadline_ms=1 WHERE request_id=$1")
        .bind(Uuid::parse_str(&request.request_id).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .get_model_planning_request(&f.owner, &f.conversation, &request.request_id)
            .await
            .unwrap()
            .status,
        "unknown"
    );
    assert!(
        reopened
            .claim_model_planning_request(&f.owner, &f.conversation, &request.request_id)
            .await
            .is_err()
    );
    assert_eq!(
        f.finish(
            &claim,
            proposal(),
            Some(ReplyUsage {
                input_tokens: 1,
                output_tokens: 1
            })
        )
        .await
        .status,
        "unknown"
    );
    assert_eq!(f.money().await, AMOUNT);
    assert_eq!(f.calls().await, 1);
    assert!(f.audit().await.consistent);
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_planning_revision_version_and_persisted_consent_are_rechecked() {
    for field in ["revision", "version", "approval"] {
        let f = Fixture::new(AMOUNT * 10, 10).await;
        let request = f.draft().await;
        f.approve(&request).await;
        match field {
            "revision" => {
                f.store
                    .append_message(&f.owner, &f.conversation, &id(), "后续消息")
                    .await
                    .unwrap();
            }
            "version" => {
                sqlx::query(
                    "UPDATE model_planning_requests SET version='unsupported' WHERE request_id=$1",
                )
                .bind(Uuid::parse_str(&request.request_id).unwrap())
                .execute(&f.pool)
                .await
                .unwrap();
            }
            _ => {
                sqlx::query("UPDATE model_planning_requests SET approval=jsonb_set(approval,'{accepted_amount}','0') WHERE request_id=$1").bind(Uuid::parse_str(&request.request_id).unwrap()).execute(&f.pool).await.unwrap();
            }
        }
        assert!(
            f.store
                .claim_model_planning_request(&f.owner, &f.conversation, &request.request_id)
                .await
                .is_err()
        );
        if field == "approval" {
            assert_eq!(f.get(&request).await.status, "queued");
            assert_eq!(f.money().await, AMOUNT);
        } else {
            assert_eq!(f.get(&request).await.status, "stale");
            assert_eq!(f.money().await, 0);
            assert_eq!(f.calls().await, 0);
        }
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_planning_cross_day_cancel_and_delete_keep_independent_receipts() {
    for dispatched in [false, true] {
        let f = Fixture::new(AMOUNT * 10, 10).await;
        let request = f.draft().await;
        f.approve(&request).await;
        let claim = if dispatched {
            Some(f.claim(&request).await)
        } else {
            None
        };
        for table in [
            "reply_money_daily",
            "model_agent_daily",
            "reply_money_reservations",
        ] {
            sqlx::query(&format!("UPDATE {table} SET day=day-1 WHERE user_id=$1"))
                .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        sqlx::query("UPDATE model_planning_requests SET budget_day=budget_day-1 WHERE user_id=$1")
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
            .execute(&f.pool)
            .await
            .unwrap();
        f.store
            .delete_conversation(&f.owner, &f.conversation)
            .await
            .unwrap();
        assert_eq!(f.money().await, if dispatched { AMOUNT } else { 0 });
        assert_eq!(f.calls().await, i64::from(dispatched));
        assert!(matches!(
            f.store
                .get_model_planning_request(&f.owner, &f.conversation, &request.request_id)
                .await,
            Err(StorageError::NotFound)
        ));
        if let Some(claim) = claim {
            assert!(
                f.store
                    .finish_model_planning_request(
                        &f.owner,
                        &f.conversation,
                        &request.request_id,
                        &claim.claim_id,
                        proposal(),
                        None
                    )
                    .await
                    .is_err()
            );
        }
        let snapshot_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM model_planning_requests WHERE user_id=$1")
                .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(snapshot_count, 0);
        sqlx::query("DELETE FROM conversations WHERE id=$1")
            .bind(Uuid::parse_str(&f.conversation).unwrap())
            .execute(&f.pool)
            .await
            .unwrap();
        let day: String =
            sqlx::query_scalar("SELECT day::text FROM reply_money_reservations WHERE user_id=$1")
                .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        let audit = f
            .store
            .audit_reply_money(&f.owner, &day, "USD", None)
            .await
            .unwrap();
        assert!(audit.consistent);
        assert!(audit.items[0].conversation_deleted);
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(f.money().await, 0);
        assert_eq!(f.calls().await, 0);
    }
}

struct ReplyPlanner(i64);
impl ReplyBudgetPlanner for ReplyPlanner {
    fn plan(&self, _: &ReplyContext) -> personal_ai_storage::StorageResult<ReplyBudget> {
        Ok(ReplyBudget {
            currency: "USD".into(),
            provider: "fixture".into(),
            price_version: "v1".into(),
            counter_version: "v1".into(),
            input_price_per_million: 1_000_000,
            output_price_per_million: 1_000_000,
            input_token_bound: 100,
            output_token_bound: 1024,
            request_limit: 1124,
            daily_limit: self.0,
        })
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_planning_shares_reply_money_and_legacy_reply_cannot_settle_agent_receipt() {
    let f = Fixture::new(AMOUNT, 10).await;
    let config = ReplyConfiguration {
        revision: id(),
        model: "fixture".into(),
    };
    let reply_id = id();
    f.store
        .reserve_budgeted_reply(
            &f.owner,
            &f.conversation,
            &reply_id,
            1,
            &config,
            Arc::new(ReplyPlanner(AMOUNT)),
        )
        .await
        .unwrap();
    let request = f.draft().await;
    assert!(
        f.store
            .approve_model_planning_request(
                &f.owner,
                &f.conversation,
                &request.request_id,
                &approval(&request)
            )
            .await
            .is_err()
    );
    assert_eq!(f.calls().await, 0);
    assert_eq!(f.money().await, 1124);
    f.store
        .cancel_reply(&f.owner, &f.conversation, &reply_id)
        .await
        .unwrap();
    f.approve(&request).await;
    assert!(
        f.store
            .reserve_budgeted_reply(
                &f.owner,
                &f.conversation,
                &id(),
                1,
                &config,
                Arc::new(ReplyPlanner(AMOUNT))
            )
            .await
            .is_err()
    );
    f.store
        .reserve_reply(&f.owner, &f.conversation, &request.request_id, 1, &config)
        .await
        .unwrap();
    f.store
        .cancel_reply(&f.owner, &f.conversation, &request.request_id)
        .await
        .unwrap();
    assert_eq!(f.money().await, AMOUNT);
    assert_eq!(f.calls().await, 1);
    let audit = f.audit().await;
    assert!(audit.consistent);
    assert_eq!(audit.counts.requests, 2);
    assert_eq!(
        audit
            .items
            .iter()
            .find(|r| r.request_id == request.request_id)
            .unwrap()
            .reply_status
            .as_deref(),
        Some("queued")
    );
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_planning_failed_commit_returns_no_authorization_or_dispatch_claim() {
    let f = Fixture::new(AMOUNT * 10, 10).await;
    let request = f.draft().await;
    let suffix = Uuid::new_v4().simple().to_string();
    let function = format!("reject_model_claim_{suffix}");
    let trigger = format!("model_commit_{suffix}");
    sqlx::query(&format!("CREATE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.user_id='{}'::uuid AND NEW.status IN ('queued','dispatching') THEN RAISE EXCEPTION 'fixture commit rejected'; END IF; RETURN NEW; END; $$",f.owner)).execute(&f.pool).await.unwrap();
    sqlx::query(&format!("CREATE CONSTRAINT TRIGGER {trigger} AFTER UPDATE ON model_planning_requests DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {function}()")).execute(&f.pool).await.unwrap();
    assert!(
        f.store
            .approve_model_planning_request(
                &f.owner,
                &f.conversation,
                &request.request_id,
                &approval(&request)
            )
            .await
            .is_err()
    );
    assert_eq!(f.get(&request).await.status, "draft");
    assert_eq!(f.money().await, 0);
    assert_eq!(f.calls().await, 0);
    sqlx::query(&format!("CREATE OR REPLACE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.user_id='{}'::uuid AND NEW.status='dispatching' THEN RAISE EXCEPTION 'fixture commit rejected'; END IF; RETURN NEW; END; $$",f.owner)).execute(&f.pool).await.unwrap();
    f.approve(&request).await;
    assert!(
        f.store
            .claim_model_planning_request(&f.owner, &f.conversation, &request.request_id)
            .await
            .is_err()
    );
    assert_eq!(f.get(&request).await.status, "queued");
    assert_eq!(f.money().await, AMOUNT);
    assert_eq!(f.calls().await, 1);
    sqlx::query(&format!(
        "DROP TRIGGER {trigger} ON model_planning_requests"
    ))
    .execute(&f.pool)
    .await
    .unwrap();
    sqlx::query(&format!("DROP FUNCTION {function}()"))
        .execute(&f.pool)
        .await
        .unwrap();
}

#[path = "model_execution.rs"]
mod execution;
