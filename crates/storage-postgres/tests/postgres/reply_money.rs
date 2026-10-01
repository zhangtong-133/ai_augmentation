use super::*;
use personal_ai_storage::{
    StorageResult,
    replies::ReplyContext,
    reply_budgets::{BudgetedReplyStore, ReplyBudget, ReplyBudgetPlanner, ReplyUsage},
};
use std::sync::Arc;

#[derive(Clone)]
struct Planner(ReplyBudget);
impl ReplyBudgetPlanner for Planner {
    fn plan(&self, context: &ReplyContext) -> StorageResult<ReplyBudget> {
        assert_ne!(context.system, "");
        assert_eq!(context.user_messages, ["问题"]);
        assert_eq!(context.configuration, configuration());
        assert_eq!(context.max_output_tokens, 1024);
        Ok(self.0.clone())
    }
}
struct MustNotPlan;
impl ReplyBudgetPlanner for MustNotPlan {
    fn plan(&self, _: &ReplyContext) -> StorageResult<ReplyBudget> {
        panic!("replay or foreign request must not count again")
    }
}
fn planner(limit: i64) -> Arc<Planner> {
    Arc::new(Planner(ReplyBudget {
        currency: "USD".into(),
        provider: "test-only".into(),
        price_version: "price-v1".into(),
        counter_version: "test-v1".into(),
        input_price_per_million: 1_000_000,
        output_price_per_million: 1_000_000,
        input_token_bound: 100,
        output_token_bound: 1024,
        request_limit: 1124,
        daily_limit: limit,
    }))
}
async fn money(pool: &sqlx::PgPool, owner: &UserId) -> i64 {
    sqlx::query_scalar(
        "SELECT COALESCE(sum(occupied),0)::bigint FROM reply_money_daily WHERE user_id=$1",
    )
    .bind(Uuid::parse_str(owner.as_str()).unwrap())
    .fetch_one(pool)
    .await
    .unwrap()
}
async fn receipt(pool: &sqlx::PgPool, request: &str) -> (i64, String) {
    sqlx::query_as("SELECT charged,settlement FROM reply_money_reservations WHERE request_id=$1")
        .bind(Uuid::parse_str(request).unwrap())
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn reserve(
    store: &PostgresStore,
    owner: &UserId,
    conversation: &str,
    request: &str,
    limit: i64,
) -> StorageResult<personal_ai_storage::replies::Reply> {
    store
        .reserve_budgeted_reply(
            owner,
            conversation,
            request,
            1,
            &configuration(),
            planner(limit),
        )
        .await
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn monetary_replay_freezes_prices_and_settles_only_once() {
    let (store, pool, owner, conversation) = fixture().await;
    let request = id();
    let (a, b) = tokio::join!(
        reserve(&store, &owner, &conversation, &request, 1124),
        reserve(&store, &owner, &conversation, &request, 1124)
    );
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(money(&pool, &owner).await, 1124);
    assert_eq!(budget(&pool, &owner).await, 1);
    store
        .append_message(&owner, &conversation, &id(), "后续")
        .await
        .unwrap();
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    reopened
        .reserve_budgeted_reply(
            &owner,
            &conversation,
            &request,
            1,
            &ReplyConfiguration {
                model: "changed".into(),
                revision: "changed".into(),
            },
            Arc::new(MustNotPlan),
        )
        .await
        .unwrap();
    store
        .claim_reply(&owner, &conversation, &request)
        .await
        .unwrap();
    let usage = Some(ReplyUsage {
        input_tokens: 80,
        output_tokens: 20,
    });
    let (a, b) = tokio::join!(
        store.finish_budgeted_reply(
            &owner,
            &conversation,
            &request,
            ReplyOutcome::Succeeded("回答".into()),
            usage
        ),
        store.finish_budgeted_reply(
            &owner,
            &conversation,
            &request,
            ReplyOutcome::Succeeded("回答".into()),
            usage
        )
    );
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(money(&pool, &owner).await, 100);
    store
        .finish_budgeted_reply(
            &owner,
            &conversation,
            &request,
            ReplyOutcome::Succeeded("late".into()),
            Some(ReplyUsage {
                input_tokens: 0,
                output_tokens: 0,
            }),
        )
        .await
        .unwrap();
    store
        .cancel_reply(&owner, &conversation, &request)
        .await
        .unwrap();
    assert_eq!(receipt(&pool, &request).await, (100, "verified".into()));
    assert_eq!(money(&pool, &owner).await, 100);
    let frozen: String = sqlx::query_scalar(
        "SELECT budget->>'price_version' FROM reply_money_reservations WHERE request_id=$1",
    )
    .bind(Uuid::parse_str(&request).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(frozen, "price-v1");
    cleanup(&pool, &owner).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn monetary_last_slot_and_count_failures_roll_back_together() {
    let (store, pool, owner, conversation) = fixture().await;
    let other = store
        .create_conversation(&owner, &id(), "另一对话")
        .await
        .unwrap()
        .id;
    store
        .append_message(&owner, &other, &id(), "问题")
        .await
        .unwrap();
    let a_id = id();
    let b_id = id();
    let (a, b) = tokio::join!(
        reserve(&store, &owner, &conversation, &a_id, 1124),
        reserve(&store, &owner, &other, &b_id, 1124)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert!(matches!(
        if a.is_ok() { &b } else { &a },
        Err(StorageError::Conflict(_))
    ));
    assert_eq!(money(&pool, &owner).await, 1124);
    assert_eq!(budget(&pool, &owner).await, 1);
    let (winner, request, loser) = if a.is_ok() {
        (&conversation, &a_id, &other)
    } else {
        (&other, &b_id, &conversation)
    };
    store.cancel_reply(&owner, winner, request).await.unwrap();
    assert_eq!(money(&pool, &owner).await, 0);
    sqlx::query("UPDATE reply_daily_budgets SET reserved=20 WHERE user_id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let rejected = id();
    assert!(matches!(
        reserve(&store, &owner, loser, &rejected, 1124).await,
        Err(StorageError::Conflict(_))
    ));
    assert_eq!(money(&pool, &owner).await, 0);
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM reply_money_reservations WHERE request_id=$1)",
    )
    .bind(Uuid::parse_str(&rejected).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!exists);
    assert!(matches!(
        store.get_reply(&owner, loser, &rejected).await,
        Err(StorageError::NotFound)
    ));
    cleanup(&pool, &owner).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn monetary_cancel_refunds_original_day_and_deletion_keeps_audit() {
    let (store, pool, owner, conversation) = fixture().await;
    let request = id();
    reserve(&store, &owner, &conversation, &request, 1124)
        .await
        .unwrap();
    for table in ["reply_money_daily", "reply_money_reservations"] {
        sqlx::query(&format!("UPDATE {table} SET day=day-1 WHERE user_id=$1"))
            .bind(Uuid::parse_str(owner.as_str()).unwrap())
            .execute(&pool)
            .await
            .unwrap();
    }
    store
        .cancel_reply(&owner, &conversation, &request)
        .await
        .unwrap();
    store
        .cancel_reply(&owner, &conversation, &request)
        .await
        .unwrap();
    assert_eq!(
        receipt(&pool, &request).await,
        (0, "cancelled_before_dispatch".into())
    );
    assert_eq!(money(&pool, &owner).await, 0);
    let sent = id();
    reserve(&store, &owner, &conversation, &sent, 2248)
        .await
        .unwrap();
    store
        .claim_reply(&owner, &conversation, &sent)
        .await
        .unwrap();
    store
        .finish_reply(&owner, &conversation, &sent, ReplyOutcome::Unknown)
        .await
        .unwrap();
    let queued = id();
    reserve(&store, &owner, &conversation, &queued, 2248)
        .await
        .unwrap();
    store
        .delete_conversation(&owner, &conversation)
        .await
        .unwrap();
    store
        .delete_conversation(&owner, &conversation)
        .await
        .unwrap();
    assert_eq!(money(&pool, &owner).await, 1124);
    assert_eq!(
        receipt(&pool, &queued).await,
        (0, "cancelled_before_dispatch".into())
    );
    sqlx::query("DELETE FROM conversations WHERE id=$1")
        .bind(Uuid::parse_str(&conversation).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(receipt(&pool, &sent).await, (1124, "retained".into()));
    assert_eq!(money(&pool, &owner).await, 1124);
    cleanup(&pool, &owner).await;
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reply_money_reservations WHERE user_id=$1")
            .bind(Uuid::parse_str(owner.as_str()).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn monetary_unknown_failed_excess_and_expired_results_retain_reservation() {
    let (store, pool, owner, conversation) = fixture().await;
    for case in 0..6 {
        let request = id();
        reserve(&store, &owner, &conversation, &request, 10000)
            .await
            .unwrap();
        store
            .claim_reply(&owner, &conversation, &request)
            .await
            .unwrap();
        let mut usage = Some(ReplyUsage {
            input_tokens: 1,
            output_tokens: 1,
        });
        let outcome = match case {
            0 => ReplyOutcome::Failed,
            1 => ReplyOutcome::Unknown,
            2 => {
                usage = None;
                ReplyOutcome::Succeeded("回答".into())
            }
            3 => {
                usage = Some(ReplyUsage {
                    input_tokens: 101,
                    output_tokens: 0,
                });
                ReplyOutcome::Succeeded("回答".into())
            }
            _ => {
                sqlx::query("UPDATE conversation_replies SET dispatched_at=clock_timestamp()-interval '121 seconds' WHERE request_id=$1").bind(Uuid::parse_str(&request).unwrap()).execute(&pool).await.unwrap();
                if case == 5 {
                    store
                        .expire_reply(&owner, &conversation, &request)
                        .await
                        .unwrap();
                }
                ReplyOutcome::Succeeded("迟到".into())
            }
        };
        store
            .finish_budgeted_reply(&owner, &conversation, &request, outcome, usage)
            .await
            .unwrap();
        assert_eq!(
            receipt(&pool, &request).await,
            (
                1124,
                if case == 3 {
                    "usage_exceeded"
                } else {
                    "retained"
                }
                .into()
            )
        );
        assert_eq!(money(&pool, &owner).await, (case + 1) * 1124);
    }
    cleanup(&pool, &owner).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn monetary_claim_cancel_and_delete_finish_races_do_not_double_refund() {
    let (store, pool, owner, conversation) = fixture().await;
    let mut expected = 0;
    for _ in 0..6 {
        let request = id();
        reserve(&store, &owner, &conversation, &request, 10000)
            .await
            .unwrap();
        let (claim, cancel) = tokio::join!(
            store.claim_reply(&owner, &conversation, &request),
            store.cancel_reply(&owner, &conversation, &request)
        );
        cancel.unwrap();
        if claim.is_ok() {
            expected += 1124;
        }
        store
            .finish_budgeted_reply(
                &owner,
                &conversation,
                &request,
                ReplyOutcome::Succeeded("迟到".into()),
                Some(ReplyUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                }),
            )
            .await
            .unwrap();
        assert_eq!(money(&pool, &owner).await, expected);
    }
    let request = id();
    reserve(&store, &owner, &conversation, &request, 10000)
        .await
        .unwrap();
    store
        .claim_reply(&owner, &conversation, &request)
        .await
        .unwrap();
    let (deleted, finished) = tokio::join!(
        store.delete_conversation(&owner, &conversation),
        store.finish_budgeted_reply(
            &owner,
            &conversation,
            &request,
            ReplyOutcome::Succeeded("回答".into()),
            Some(ReplyUsage {
                input_tokens: 10,
                output_tokens: 10
            })
        )
    );
    deleted.unwrap();
    assert!(finished.is_ok() || matches!(finished, Err(StorageError::NotFound)));
    let (charged, reason) = receipt(&pool, &request).await;
    assert!(matches!(
        (charged, reason.as_str()),
        (20, "verified") | (1124, "retained")
    ));
    assert_eq!(money(&pool, &owner).await, expected + charged);
    cleanup(&pool, &owner).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn monetary_invalid_configuration_and_foreign_access_leave_no_ledger() {
    let (store, pool, owner, conversation) = fixture().await;
    for case in 0..7 {
        let mut invalid = (*planner(1124)).clone();
        match case {
            0 => invalid.0.currency = "usd".into(),
            1 => invalid.0.provider = String::new(),
            2 => invalid.0.output_token_bound = 1025,
            3 => invalid.0.input_token_bound = 0,
            4 => invalid.0.daily_limit = 0,
            5 => invalid.0.request_limit = 1123,
            _ => invalid.0.daily_limit = 1123,
        }
        assert!(
            store
                .reserve_budgeted_reply(
                    &owner,
                    &conversation,
                    &id(),
                    1,
                    &configuration(),
                    Arc::new(invalid)
                )
                .await
                .is_err()
        );
    }
    assert_eq!(money(&pool, &owner).await, 0);
    assert_eq!(budget(&pool, &owner).await, 0);
    let (_, foreign_pool, foreign, _) = fixture().await;
    assert!(matches!(
        store
            .reserve_budgeted_reply(
                &foreign,
                &conversation,
                &id(),
                1,
                &configuration(),
                Arc::new(MustNotPlan)
            )
            .await,
        Err(StorageError::NotFound)
    ));
    let request = id();
    reserve(&store, &owner, &conversation, &request, 1124)
        .await
        .unwrap();
    assert!(matches!(
        store
            .finish_budgeted_reply(
                &foreign,
                &conversation,
                &request,
                ReplyOutcome::Failed,
                None
            )
            .await,
        Err(StorageError::NotFound)
    ));
    assert_eq!(money(&pool, &owner).await, 1124);
    cleanup(&pool, &owner).await;
    cleanup(&foreign_pool, &foreign).await;
}

#[path = "reply_dispatch.rs"]
mod dispatch;

#[path = "reply_operations.rs"]
mod operations;
