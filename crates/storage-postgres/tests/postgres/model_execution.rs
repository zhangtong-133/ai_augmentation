use super::*;
use personal_ai_storage::{
    model_execution::{
        ModelExecutionClaim, ModelExecutionConfiguration, ModelExecutionOutcome,
        ModelExecutionRequest, ModelExecutionStore,
    },
    tool_calls::{NewToolCall, ToolCallStore},
};
const EXECUTION_AMOUNT: i64 = 1224;
async fn planned(fixture: &Fixture, search_count: usize) -> ModelPlanningRequest {
    let draft = fixture.draft().await;
    fixture.approve(&draft).await;
    let claim = fixture.claim(&draft).await;
    let searches: Vec<_> = (0..search_count)
        .map(|i| serde_json::json!({"query":format!("research {i}"),"limit":5}))
        .collect();
    fixture
        .finish(
            &claim,
            ModelPlanningOutcome::Proposed(
                serde_json::to_vec(&serde_json::json!({"searches":searches})).unwrap(),
            ),
            None,
        )
        .await
}
fn config(fixture: &Fixture) -> ModelExecutionConfiguration {
    let mut answer = fixture.configuration.budget.clone();
    answer.configuration_version = id();
    answer.output_token_bound = 1024;
    let mut embedding = answer.clone();
    embedding.configuration_version = id();
    embedding.output_price_per_million = 0;
    embedding.output_token_bound = 0;
    ModelExecutionConfiguration {
        version: answer.configuration_version.clone(),
        embedding,
        answer,
        limits: AgentBudgetLimits {
            phase_amount: 2000,
            ..fixture.configuration.limits
        },
    }
}
fn consent(request: &ModelExecutionRequest) -> AgentQuoteApproval {
    AgentQuoteApproval {
        digest: request.digest.clone(),
        accepted_currency: request.currency.clone(),
        accepted_amount: request.amount,
        accepted_calls: request.calls,
        acknowledge_cost: true,
    }
}
async fn draft(
    fixture: &Fixture,
    search_count: usize,
) -> (ModelExecutionRequest, ModelExecutionConfiguration) {
    let planning = planned(fixture, search_count).await;
    let configuration = config(fixture);
    fixture
        .store
        .register_model_execution_configuration(&configuration)
        .await
        .unwrap();
    let request = fixture
        .store
        .create_model_execution_request(
            &fixture.owner,
            &fixture.conversation,
            &planning.request_id,
            &configuration.version,
        )
        .await
        .unwrap();
    (request, configuration)
}
async fn approve(fixture: &Fixture, request: &ModelExecutionRequest) {
    assert!(
        fixture
            .store
            .approve_model_execution_request(
                &fixture.owner,
                &fixture.conversation,
                &request.request_id,
                &consent(request)
            )
            .await
            .unwrap()
            .started
    );
}
async fn tool_count(fixture: &Fixture) -> i64 {
    sqlx::query_scalar(
        "SELECT COALESCE(sum(used),0)::bigint FROM tool_daily_budgets WHERE user_id=$1",
    )
    .bind(Uuid::parse_str(fixture.owner.as_str()).unwrap())
    .fetch_one(&fixture.pool)
    .await
    .unwrap()
}
async fn claim(fixture: &Fixture, request: &ModelExecutionRequest, i: u32) -> ModelExecutionClaim {
    fixture
        .store
        .claim_model_execution_step(
            &fixture.owner,
            &fixture.conversation,
            &request.request_id,
            i,
        )
        .await
        .unwrap()
}
async fn finish(
    fixture: &Fixture,
    configuration: &ModelExecutionClaim,
    outcome: ModelExecutionOutcome,
    usage: Option<ReplyUsage>,
) -> ModelExecutionRequest {
    fixture
        .store
        .finish_model_execution_step(
            &fixture.owner,
            &fixture.conversation,
            &configuration.request.request_id,
            &configuration.claim_id,
            outcome,
            usage,
        )
        .await
        .unwrap()
}
async fn cancel(fixture: &Fixture, request: &ModelExecutionRequest) -> ModelExecutionRequest {
    fixture
        .store
        .cancel_model_execution_request(&fixture.owner, &fixture.conversation, &request.request_id)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)] // 单个真实事务生命周期验收。
async fn execution_exact_consent_sequential_claims_and_settlement_are_durable() {
    let fixture = Fixture::new(20000, 100).await;
    let (request, configuration) = draft(&fixture, 1).await;
    assert_eq!(
        (request.amount, fixture.money().await, fixture.calls().await),
        (EXECUTION_AMOUNT, AMOUNT, 1)
    );
    let mut wrong = consent(&request);
    wrong.accepted_amount -= 1;
    assert!(
        fixture
            .store
            .approve_model_execution_request(
                &fixture.owner,
                &fixture.conversation,
                &request.request_id,
                &wrong
            )
            .await
            .is_err()
    );
    let approval = consent(&request);
    let (first, second) = tokio::join!(
        fixture.store.approve_model_execution_request(
            &fixture.owner,
            &fixture.conversation,
            &request.request_id,
            &approval
        ),
        fixture.store.approve_model_execution_request(
            &fixture.owner,
            &fixture.conversation,
            &request.request_id,
            &approval
        )
    );
    assert_ne!(first.unwrap().started, second.unwrap().started);
    assert_eq!(
        (
            fixture.money().await,
            fixture.calls().await,
            tool_count(&fixture).await
        ),
        (AMOUNT + EXECUTION_AMOUNT, 3, 1)
    );
    assert!(
        fixture
            .store
            .claim_model_execution_step(
                &fixture.owner,
                &fixture.conversation,
                &request.request_id,
                1
            )
            .await
            .is_err()
    );
    let (first, second) = tokio::join!(
        fixture.store.claim_model_execution_step(
            &fixture.owner,
            &fixture.conversation,
            &request.request_id,
            0
        ),
        fixture.store.claim_model_execution_step(
            &fixture.owner,
            &fixture.conversation,
            &request.request_id,
            0
        )
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let search = first.or(second).unwrap();
    assert_eq!(search.query.as_ref().unwrap().query, "research 0");
    assert_eq!(tool_count(&fixture).await, 1);
    assert!(
        fixture
            .store
            .finish_model_execution_step(
                &fixture.owner,
                &fixture.conversation,
                &request.request_id,
                &id(),
                ModelExecutionOutcome::Failed,
                None
            )
            .await
            .is_err()
    );
    finish(
        &fixture,
        &search,
        ModelExecutionOutcome::Succeeded { output_bytes: 10 },
        Some(ReplyUsage {
            input_tokens: 20,
            output_tokens: 0,
        }),
    )
    .await;
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert!(
        reopened
            .claim_model_execution_step(
                &fixture.owner,
                &fixture.conversation,
                &request.request_id,
                0
            )
            .await
            .is_err()
    );
    let answer = claim(&fixture, &request, 1).await;
    assert!(answer.query.is_none());
    assert_eq!(answer.snapshot.revision, 1);
    let result = finish(
        &fixture,
        &answer,
        ModelExecutionOutcome::Succeeded { output_bytes: 10 },
        Some(ReplyUsage {
            input_tokens: 100,
            output_tokens: 20,
        }),
    )
    .await;
    assert_eq!(result.status, "succeeded");
    finish(
        &fixture,
        &answer,
        ModelExecutionOutcome::Succeeded { output_bytes: 0 },
        Some(ReplyUsage {
            input_tokens: 0,
            output_tokens: 0,
        }),
    )
    .await;
    assert_eq!(
        (
            fixture.money().await,
            fixture.calls().await,
            tool_count(&fixture).await
        ),
        (AMOUNT + 140, 3, 1)
    );
    assert_eq!(fixture.audit().await.difference_micro, "0");
    assert_eq!(
        fixture
            .store
            .create_model_execution_request(
                &fixture.owner,
                &fixture.conversation,
                &request.request_id,
                &configuration.version
            )
            .await
            .unwrap(),
        result
    );
    let tool = fixture
        .store
        .get_tool_call(&fixture.owner, &search.call_id)
        .await
        .unwrap();
    assert_eq!(tool.status, "succeeded");
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn execution_requires_successful_owned_source_and_live_revision() {
    let fixture = Fixture::new(20000, 100).await;
    let planning = fixture.draft().await;
    let configuration = config(&fixture);
    fixture
        .store
        .register_model_execution_configuration(&configuration)
        .await
        .unwrap();
    assert!(
        fixture
            .store
            .create_model_execution_request(
                &fixture.owner,
                &fixture.conversation,
                &planning.request_id,
                &configuration.version
            )
            .await
            .is_err()
    );
    let (request, _) = draft(&fixture, 1).await;
    assert!(matches!(
        fixture
            .store
            .create_model_execution_request(
                &fixture.foreign,
                &fixture.conversation,
                &request.request_id,
                &configuration.version
            )
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        fixture
            .store
            .approve_model_execution_request(
                &fixture.foreign,
                &fixture.conversation,
                &request.request_id,
                &consent(&request)
            )
            .await,
        Err(StorageError::NotFound)
    ));
    approve(&fixture, &request).await;
    fixture
        .store
        .append_message(&fixture.owner, &fixture.conversation, &id(), "changed")
        .await
        .unwrap();
    assert!(
        fixture
            .store
            .claim_model_execution_step(
                &fixture.owner,
                &fixture.conversation,
                &request.request_id,
                0
            )
            .await
            .is_err()
    );
    let stale = fixture
        .store
        .get_model_execution_request(&fixture.owner, &fixture.conversation, &request.request_id)
        .await
        .unwrap();
    assert_eq!(stale.status, "stale");
    assert!(stale.searches.is_none());
    assert_eq!(
        (
            fixture.money().await,
            fixture.calls().await,
            tool_count(&fixture).await
        ),
        (AMOUNT, 1, 0)
    );
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn execution_partial_cancel_and_delete_retain_only_dispatched_attempts() {
    let fixture = Fixture::new(20000, 100).await;
    let (request, _) = draft(&fixture, 3).await;
    approve(&fixture, &request).await;
    let search = claim(&fixture, &request, 0).await;
    assert_eq!(cancel(&fixture, &request).await.status, "cancelled");
    cancel(&fixture, &request).await;
    finish(
        &fixture,
        &search,
        ModelExecutionOutcome::Succeeded { output_bytes: 0 },
        Some(ReplyUsage {
            input_tokens: 0,
            output_tokens: 0,
        }),
    )
    .await;
    assert_eq!(
        (
            fixture.money().await,
            fixture.calls().await,
            tool_count(&fixture).await
        ),
        (AMOUNT + 100, 2, 1)
    );
    fixture
        .store
        .delete_conversation(&fixture.owner, &fixture.conversation)
        .await
        .unwrap();
    sqlx::query("DELETE FROM conversations WHERE id=$1")
        .bind(Uuid::parse_str(&fixture.conversation).unwrap())
        .execute(&fixture.pool)
        .await
        .unwrap();
    assert_eq!(fixture.money().await, AMOUNT + 100);
    assert_eq!(fixture.audit().await.difference_micro, "0");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM model_execution_requests WHERE user_id=$1")
            .bind(Uuid::parse_str(fixture.owner.as_str()).unwrap())
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    let fixture = Fixture::new(20000, 100).await;
    let (request, _) = draft(&fixture, 2).await;
    approve(&fixture, &request).await;
    fixture
        .store
        .delete_conversation(&fixture.owner, &fixture.conversation)
        .await
        .unwrap();
    assert_eq!(
        (
            fixture.money().await,
            fixture.calls().await,
            tool_count(&fixture).await
        ),
        (AMOUNT, 1, 0)
    );
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn execution_unknown_exceeded_and_expired_calls_do_not_retry() {
    for case in 0..4 {
        let fixture = Fixture::new(20000, 100).await;
        let (request, configuration) = draft(&fixture, 2).await;
        approve(&fixture, &request).await;
        let search = claim(&fixture, &request, 0).await;
        if case == 2 {
            sqlx::query("UPDATE model_execution_requests SET data=jsonb_set(data,'{steps,0,deadline}','1') WHERE user_id=$1 AND request_id=$2").bind(Uuid::parse_str(fixture.owner.as_str()).unwrap()).bind(Uuid::parse_str(&request.request_id).unwrap()).execute(&fixture.pool).await.unwrap();
        }
        let usage = if case == 3 {
            Some(ReplyUsage {
                input_tokens: 101,
                output_tokens: 0,
            })
        } else {
            None
        };
        let outcome = if case == 0 {
            ModelExecutionOutcome::Failed
        } else if case == 1 {
            ModelExecutionOutcome::Unknown
        } else {
            ModelExecutionOutcome::Succeeded { output_bytes: 0 }
        };
        let terminal = finish(&fixture, &search, outcome, usage).await;
        assert!(matches!(terminal.status.as_str(), "failed" | "unknown"));
        assert_eq!(
            (
                fixture.money().await,
                fixture.calls().await,
                tool_count(&fixture).await
            ),
            (AMOUNT + 100, 2, 1)
        );
        assert!(
            fixture
                .store
                .claim_model_execution_step(
                    &fixture.owner,
                    &fixture.conversation,
                    &request.request_id,
                    0
                )
                .await
                .is_err()
        );
        assert!(
            fixture
                .store
                .claim_model_execution_step(
                    &fixture.owner,
                    &fixture.conversation,
                    &request.request_id,
                    1
                )
                .await
                .is_err()
        );
        // 迟到越界也持久化停用，但不覆盖已结算金额。
        finish(
            &fixture,
            &search,
            ModelExecutionOutcome::Failed,
            Some(ReplyUsage {
                input_tokens: 101,
                output_tokens: 0,
            }),
        )
        .await;
        assert!(
            fixture
                .store
                .check_model_execution_configuration(&configuration)
                .await
                .is_err()
        );
        assert_eq!(fixture.money().await, AMOUNT + 100);
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn execution_last_money_model_or_tool_slot_is_atomic_and_shared() {
    for axis in 0..3 {
        let fixture = Fixture::new(20000, 100).await;
        let planning = planned(&fixture, 1).await;
        let other_planning = planned(&fixture, 1).await;
        let mut configuration = config(&fixture);
        match axis {
            0 => configuration.limits.daily_amount = 2 * AMOUNT + EXECUTION_AMOUNT,
            1 => configuration.limits.daily_model_calls = 4,
            _ => configuration.limits.daily_tool_calls = 1,
        }
        fixture
            .store
            .register_model_execution_configuration(&configuration)
            .await
            .unwrap();
        let first = fixture
            .store
            .create_model_execution_request(
                &fixture.owner,
                &fixture.conversation,
                &planning.request_id,
                &configuration.version,
            )
            .await
            .unwrap();
        let second = fixture
            .store
            .create_model_execution_request(
                &fixture.owner,
                &fixture.conversation,
                &other_planning.request_id,
                &configuration.version,
            )
            .await
            .unwrap();
        let ac = consent(&first);
        let bc = consent(&second);
        let (first_result, second_result) = tokio::join!(
            fixture.store.approve_model_execution_request(
                &fixture.owner,
                &fixture.conversation,
                &first.request_id,
                &ac
            ),
            fixture.store.approve_model_execution_request(
                &fixture.owner,
                &fixture.conversation,
                &second.request_id,
                &bc
            )
        );
        assert_ne!(first_result.is_ok(), second_result.is_ok());
        assert_eq!(
            (
                fixture.money().await,
                fixture.calls().await,
                tool_count(&fixture).await
            ),
            (2 * AMOUNT + EXECUTION_AMOUNT, 4, 1)
        );
        cancel(
            &fixture,
            if first_result.is_ok() {
                &first
            } else {
                &second
            },
        )
        .await;
        assert_eq!(
            (
                fixture.money().await,
                fixture.calls().await,
                tool_count(&fixture).await
            ),
            (2 * AMOUNT, 2, 0)
        );
    }
    let fixture = Fixture::new(20000, 100).await;
    let (request, _) = draft(&fixture, 1).await;
    // 普通工具与第二阶段共用 100 次账本；预留不能再次占用最后一个名额。
    sqlx::query("INSERT INTO tool_daily_budgets(user_id,day,used) VALUES($1,(clock_timestamp() AT TIME ZONE 'UTC')::date,100)").bind(Uuid::parse_str(fixture.owner.as_str()).unwrap()).execute(&fixture.pool).await.unwrap();
    assert!(
        fixture
            .store
            .approve_model_execution_request(
                &fixture.owner,
                &fixture.conversation,
                &request.request_id,
                &consent(&request)
            )
            .await
            .is_err()
    );
    assert_eq!((fixture.money().await, fixture.calls().await), (AMOUNT, 1));
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn execution_configuration_stop_cross_day_and_expiry_refund_are_persistent() {
    let fixture = Fixture::new(20000, 100).await;
    let (request, configuration) = draft(&fixture, 1).await;
    let mut changed = configuration.clone();
    changed.answer.price_version = "changed".into();
    assert!(
        fixture
            .store
            .register_model_execution_configuration(&changed)
            .await
            .is_err()
    );
    approve(&fixture, &request).await;
    fixture
        .store
        .disable_model_execution_configuration(&configuration.version)
        .await
        .unwrap();
    assert!(
        fixture
            .store
            .register_model_execution_configuration(&configuration)
            .await
            .is_err()
    );
    assert!(
        fixture
            .store
            .claim_model_execution_step(
                &fixture.owner,
                &fixture.conversation,
                &request.request_id,
                0
            )
            .await
            .is_err()
    );
    cancel(&fixture, &request).await;
    let fixture = Fixture::new(20000, 100).await;
    let (request, _) = draft(&fixture, 1).await;
    approve(&fixture, &request).await;
    for (table, col) in [
        ("reply_money_daily", "day"),
        ("reply_money_reservations", "day"),
        ("model_agent_daily", "day"),
        ("tool_daily_budgets", "day"),
    ] {
        sqlx::query(&format!(
            "UPDATE {table} SET {col}={col}-1 WHERE user_id=$1"
        ))
        .bind(Uuid::parse_str(fixture.owner.as_str()).unwrap())
        .execute(&fixture.pool)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE model_execution_requests SET data=jsonb_set(jsonb_set(data,'{day}',to_jsonb(((data->>'day')::date-1)::text)),'{request,expires_at_unix_ms}','1') WHERE user_id=$1").bind(Uuid::parse_str(fixture.owner.as_str()).unwrap()).execute(&fixture.pool).await.unwrap();
    let expired = fixture
        .store
        .get_model_execution_request(&fixture.owner, &fixture.conversation, &request.request_id)
        .await
        .unwrap();
    assert_eq!(expired.status, "expired");
    assert_eq!(
        (
            fixture.money().await,
            fixture.calls().await,
            tool_count(&fixture).await
        ),
        (AMOUNT, 1, 0)
    );
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn execution_cancel_claim_race_and_tool_id_replay_never_resend() {
    let fixture = Fixture::new(20000, 100).await;
    let (request, _) = draft(&fixture, 1).await;
    approve(&fixture, &request).await;
    let (taken, stopped) = tokio::join!(
        fixture.store.claim_model_execution_step(
            &fixture.owner,
            &fixture.conversation,
            &request.request_id,
            0
        ),
        fixture.store.cancel_model_execution_request(
            &fixture.owner,
            &fixture.conversation,
            &request.request_id
        )
    );
    assert_eq!(stopped.unwrap().status, "cancelled");
    assert_eq!(
        fixture.money().await,
        AMOUNT + if taken.is_ok() { 100 } else { 0 }
    );
    let fixture = Fixture::new(20000, 100).await;
    let (request, _) = draft(&fixture, 1).await;
    approve(&fixture, &request).await;
    let search = claim(&fixture, &request, 0).await;
    let call = personal_ai_agent_core::tool_execution::prepare_tool_call(
        &search.call_id,
        "knowledge_search",
        &personal_ai_tools::ToolRequest {
            arguments_json: serde_json::to_string(search.query.as_ref().unwrap()).unwrap(),
        },
    )
    .unwrap();
    assert!(matches!(
        fixture
            .store
            .start_tool_call(&fixture.owner, &NewToolCall { ..call })
            .await
            .unwrap(),
        personal_ai_storage::tool_calls::ToolCallStart::Existing(_)
    ));
    assert_eq!(tool_count(&fixture).await, 1);
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn execution_commit_failure_returns_no_authorization_or_claim() {
    let fixture = Fixture::new(20000, 100).await;
    let (request, _) = draft(&fixture, 1).await;
    for status in ["queued", "running"] {
        let function = format!("execution_fail_{}", Uuid::new_v4().simple());
        let trigger = format!("execution_trigger_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.request_id='{}'::uuid AND NEW.data->'request'->>'status'='{status}' THEN RAISE EXCEPTION 'fixture commit failure'; END IF; RETURN NEW; END $$", request.request_id)).execute(&fixture.pool).await.unwrap();
        sqlx::query(&format!("CREATE CONSTRAINT TRIGGER {trigger} AFTER UPDATE ON model_execution_requests DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {function}()" )).execute(&fixture.pool).await.unwrap();
        if status == "queued" {
            assert!(
                fixture
                    .store
                    .approve_model_execution_request(
                        &fixture.owner,
                        &fixture.conversation,
                        &request.request_id,
                        &consent(&request)
                    )
                    .await
                    .is_err()
            );
            assert_eq!(
                (
                    fixture.money().await,
                    fixture.calls().await,
                    tool_count(&fixture).await
                ),
                (AMOUNT, 1, 0)
            );
        } else {
            assert!(
                fixture
                    .store
                    .claim_model_execution_step(
                        &fixture.owner,
                        &fixture.conversation,
                        &request.request_id,
                        0
                    )
                    .await
                    .is_err()
            );
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tool_calls WHERE user_id=$1")
                .bind(Uuid::parse_str(fixture.owner.as_str()).unwrap())
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
            assert_eq!(count, 0);
        }
        sqlx::query(&format!(
            "DROP TRIGGER {trigger} ON model_execution_requests"
        ))
        .execute(&fixture.pool)
        .await
        .unwrap();
        sqlx::query(&format!("DROP FUNCTION {function}()"))
            .execute(&fixture.pool)
            .await
            .unwrap();
        if status == "queued" {
            approve(&fixture, &request).await;
        } else {
            claim(&fixture, &request, 0).await;
        }
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn execution_rechecks_persisted_approval_and_receipt_before_claim() {
    for field in ["approval", "receipt"] {
        let fixture = Fixture::new(20000, 100).await;
        let (request, _) = draft(&fixture, 1).await;
        approve(&fixture, &request).await;
        if field == "approval" {
            sqlx::query("UPDATE model_execution_requests SET data=jsonb_set(data,'{approval,accepted_amount}','0') WHERE user_id=$1 AND request_id=$2").bind(Uuid::parse_str(fixture.owner.as_str()).unwrap()).bind(Uuid::parse_str(&request.request_id).unwrap()).execute(&fixture.pool).await.unwrap();
        } else {
            sqlx::query("UPDATE reply_money_reservations SET reserved=reserved+1 WHERE user_id=$1 AND request_kind='model_execution'").bind(Uuid::parse_str(fixture.owner.as_str()).unwrap()).execute(&fixture.pool).await.unwrap();
        }
        assert!(
            fixture
                .store
                .claim_model_execution_step(
                    &fixture.owner,
                    &fixture.conversation,
                    &request.request_id,
                    0
                )
                .await
                .is_err()
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tool_calls WHERE user_id=$1")
            .bind(Uuid::parse_str(fixture.owner.as_str()).unwrap())
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(fixture.money().await, AMOUNT + EXECUTION_AMOUNT);
    }
}
