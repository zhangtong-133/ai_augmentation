use super::*;
use personal_ai_agent_core::{
    BoxFuture,
    reply_executor::{BudgetedReplyExecutor, ReplyCompletion, ReplySendError, ReplySender},
};
use personal_ai_storage::{
    StorageResult,
    replies::ReplyContext,
    reply_budgets::{
        BudgetedReplyStore, ReplyBudget, ReplyBudgetPlanner, ReplyDispatchStore, ReplyUsage,
    },
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

fn budget() -> ReplyBudget {
    ReplyBudget {
        currency: "USD".into(),
        provider: "test".into(),
        price_version: "test".into(),
        counter_version: "test".into(),
        input_price_per_million: 1_000_000,
        output_price_per_million: 1_000_000,
        input_token_bound: 100,
        output_token_bound: 1024,
        request_limit: 1124,
        daily_limit: 10000,
    }
}
struct Planner;
impl ReplyBudgetPlanner for Planner {
    fn plan(&self, _: &ReplyContext) -> StorageResult<ReplyBudget> {
        Ok(budget())
    }
}
struct Sender {
    calls: AtomicUsize,
    mode: u8,
    entered: Notify,
    release: Notify,
}
impl Sender {
    fn new(mode: u8) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            mode,
            entered: Notify::new(),
            release: Notify::new(),
        })
    }
}
impl ReplySender for Sender {
    fn send<'a>(
        &'a self,
        _: &'a ReplyContext,
        frozen: &'a ReplyBudget,
    ) -> BoxFuture<'a, Result<ReplyCompletion, ReplySendError>> {
        Box::pin(async move {
            assert_eq!(*frozen, budget());
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            if self.mode == 6 {
                self.release.notified().await;
            }
            if self.mode == 7 {
                std::future::pending::<()>().await;
            }
            match self.mode {
                1 => Err(ReplySendError::Unknown),
                2 => Err(ReplySendError::InvalidResponse),
                3 => Err(ReplySendError::ContractViolation),
                _ => Ok(ReplyCompletion {
                    content: "本地供应商夹具".into(),
                    usage: if self.mode == 4 {
                        None
                    } else {
                        Some(ReplyUsage {
                            input_tokens: if self.mode == 5 { 101 } else { 10 },
                            output_tokens: 20,
                        })
                    },
                }),
            }
        })
    }
}
async fn config(store: &PostgresStore) -> ReplyConfiguration {
    let config = ReplyConfiguration {
        model: "test-snapshot".into(),
        revision: Uuid::new_v4().to_string(),
    };
    store
        .register_reply_configuration(&config, &budget(), 4_102_444_800_000)
        .await
        .unwrap();
    config
}
async fn reserve(
    store: &PostgresStore,
    owner: &User,
    conversation: &str,
    config: &ReplyConfiguration,
) -> String {
    let request = Uuid::new_v4().to_string();
    store
        .reserve_budgeted_reply(
            &owner.id,
            conversation,
            &request,
            1,
            config,
            Arc::new(Planner),
        )
        .await
        .unwrap();
    request
}
async fn charged(pool: &sqlx::PgPool, request: &str) -> i64 {
    sqlx::query_scalar("SELECT charged FROM reply_money_reservations WHERE request_id=$1")
        .bind(Uuid::parse_str(request).unwrap())
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn cleanup(pool: &sqlx::PgPool, owner: &User, config: &ReplyConfiguration) {
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.id.as_str()).unwrap())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM reply_configurations WHERE revision=$1")
        .bind(&config.revision)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn provider_executor_claims_once_and_settles_verified_usage() {
    let (url, store, pool, owner, conversation) = fixture().await;
    let config = config(&store).await;
    let request = reserve(&store, &owner, &conversation, &config).await;
    let sender = Sender::new(0);
    let a = BudgetedReplyExecutor::new(store.clone(), sender.clone(), config.clone());
    let b = BudgetedReplyExecutor::new(
        Arc::new(PostgresStore::connect(&url).await.unwrap()),
        sender.clone(),
        config.clone(),
    );
    tokio::join!(a.tick(), b.tick());
    a.tick().await;
    assert_eq!(sender.calls.load(Ordering::SeqCst), 1);
    let reply = store
        .get_reply(&owner.id, &conversation, &request)
        .await
        .unwrap();
    assert_eq!(reply.status, ReplyStatus::Succeeded);
    assert_eq!(reply.output.as_deref(), Some("本地供应商夹具"));
    assert_eq!(charged(&pool, &request).await, 30);
    cleanup(&pool, &owner, &config).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn provider_failures_keep_money_and_contract_violations_disable_durably() {
    for mode in 1..=5 {
        let (url, store, pool, owner, conversation) = fixture().await;
        let config = config(&store).await;
        let request = reserve(&store, &owner, &conversation, &config).await;
        let sender = Sender::new(mode);
        let executor = BudgetedReplyExecutor::new(store.clone(), sender.clone(), config.clone());
        executor.tick().await;
        executor.tick().await;
        assert_eq!(sender.calls.load(Ordering::SeqCst), 1);
        assert_eq!(charged(&pool, &request).await, 1124);
        assert_eq!(
            store
                .get_reply(&owner.id, &conversation, &request)
                .await
                .unwrap()
                .status,
            match mode {
                2 => ReplyStatus::Failed,
                4 => ReplyStatus::Succeeded,
                _ => ReplyStatus::Unknown,
            }
        );
        if mode == 3 || mode == 5 {
            let reopened = Arc::new(PostgresStore::connect(&url).await.unwrap());
            assert!(
                reopened
                    .check_reply_configuration(&config, &budget())
                    .await
                    .is_err()
            );
            let next = reserve(&store, &owner, &conversation, &config).await;
            BudgetedReplyExecutor::new(reopened, sender.clone(), config.clone())
                .tick()
                .await;
            assert_eq!(sender.calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                store
                    .get_reply(&owner.id, &conversation, &next)
                    .await
                    .unwrap()
                    .status,
                ReplyStatus::Queued
            );
        }
        cleanup(&pool, &owner, &config).await;
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn disabled_and_legacy_requests_never_send_and_expiration_never_retries() {
    let (_, store, pool, owner, conversation) = fixture().await;
    let config = config(&store).await;
    let sender = Sender::new(0);
    let executor = BudgetedReplyExecutor::new(store.clone(), sender.clone(), config.clone());
    let legacy = Uuid::new_v4().to_string();
    store
        .reserve_reply(&owner.id, &conversation, &legacy, 1, &config)
        .await
        .unwrap();
    executor.tick().await;
    assert_eq!(sender.calls.load(Ordering::SeqCst), 0);
    store
        .cancel_reply(&owner.id, &conversation, &legacy)
        .await
        .unwrap();
    let request = reserve(&store, &owner, &conversation, &config).await;
    store
        .claim_budgeted_reply(&owner.id, &conversation, &request)
        .await
        .unwrap();
    sqlx::query("UPDATE conversation_replies SET dispatched_at=clock_timestamp()-interval '121 seconds' WHERE request_id=$1").bind(Uuid::parse_str(&request).unwrap()).execute(&pool).await.unwrap();
    executor.tick().await;
    assert_eq!(
        store
            .get_reply(&owner.id, &conversation, &request)
            .await
            .unwrap()
            .status,
        ReplyStatus::Unknown
    );
    assert_eq!(charged(&pool, &request).await, 1124);
    let request = reserve(&store, &owner, &conversation, &config).await;
    store
        .disable_reply_configuration(&config.revision)
        .await
        .unwrap();
    executor.tick().await;
    assert_eq!(
        store
            .get_reply(&owner.id, &conversation, &request)
            .await
            .unwrap()
            .status,
        ReplyStatus::Queued
    );
    assert_eq!(sender.calls.load(Ordering::SeqCst), 0);
    cleanup(&pool, &owner, &config).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn cancellation_and_deletion_discard_late_provider_output_without_refund() {
    for delete in [false, true] {
        let (_, store, pool, owner, conversation) = fixture().await;
        let config = config(&store).await;
        let request = reserve(&store, &owner, &conversation, &config).await;
        let sender = Sender::new(6);
        let executor = Arc::new(BudgetedReplyExecutor::new(
            store.clone(),
            sender.clone(),
            config.clone(),
        ));
        let worker = tokio::spawn({
            let executor = executor.clone();
            async move {
                executor.tick().await;
            }
        });
        sender.entered.notified().await;
        if delete {
            store
                .delete_conversation(&owner.id, &conversation)
                .await
                .unwrap();
        } else {
            store
                .cancel_reply(&owner.id, &conversation, &request)
                .await
                .unwrap();
        }
        sender.release.notify_one();
        worker.await.unwrap();
        executor.tick().await;
        assert_eq!(sender.calls.load(Ordering::SeqCst), 1);
        assert_eq!(charged(&pool, &request).await, 1124);
        if !delete {
            let reply = store
                .get_reply(&owner.id, &conversation, &request)
                .await
                .unwrap();
            assert_eq!(reply.status, ReplyStatus::Cancelled);
            assert!(reply.output.is_none());
        }
        cleanup(&pool, &owner, &config).await;
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn provider_timeout_is_unknown_without_a_second_attempt() {
    let (_, store, pool, owner, conversation) = fixture().await;
    let config = config(&store).await;
    let request = reserve(&store, &owner, &conversation, &config).await;
    let sender = Sender::new(7);
    let executor = BudgetedReplyExecutor::new(store.clone(), sender.clone(), config.clone());
    executor.tick().await;
    executor.tick().await;
    assert_eq!(sender.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        store
            .get_reply(&owner.id, &conversation, &request)
            .await
            .unwrap()
            .status,
        ReplyStatus::Unknown
    );
    assert_eq!(charged(&pool, &request).await, 1124);
    cleanup(&pool, &owner, &config).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn completion_write_failure_expires_without_resending() {
    let (_, store, pool, owner, conversation) = fixture().await;
    let config = config(&store).await;
    let request = reserve(&store, &owner, &conversation, &config).await;
    let constraint = format!("test_completion_{}", Uuid::new_v4().simple());
    // 仅对本请求注入终态写入故障，其他并行测试不受影响。
    sqlx::query(&format!("ALTER TABLE conversation_replies ADD CONSTRAINT {constraint} CHECK (request_id <> '{request}'::uuid OR status <> 'succeeded')")).execute(&pool).await.unwrap();
    let sender = Sender::new(0);
    let executor = BudgetedReplyExecutor::new(store.clone(), sender.clone(), config.clone());
    executor.tick().await;
    executor.tick().await;
    assert_eq!(sender.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        store
            .get_reply(&owner.id, &conversation, &request)
            .await
            .unwrap()
            .status,
        ReplyStatus::Dispatching
    );
    let pending: bool = sqlx::query_scalar(
        "SELECT charged IS NULL FROM reply_money_reservations WHERE request_id=$1",
    )
    .bind(Uuid::parse_str(&request).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(pending);
    sqlx::query(&format!(
        "ALTER TABLE conversation_replies DROP CONSTRAINT {constraint}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE conversation_replies SET dispatched_at=clock_timestamp()-interval '121 seconds' WHERE request_id=$1").bind(Uuid::parse_str(&request).unwrap()).execute(&pool).await.unwrap();
    executor.tick().await;
    assert_eq!(sender.calls.load(Ordering::SeqCst), 1);
    assert_eq!(charged(&pool, &request).await, 1124);
    assert_eq!(
        store
            .get_reply(&owner.id, &conversation, &request)
            .await
            .unwrap()
            .status,
        ReplyStatus::Unknown
    );
    cleanup(&pool, &owner, &config).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn disable_write_failure_halts_locally_and_retries_only_persistence() {
    let (_, store, pool, owner, conversation) = fixture().await;
    let config = config(&store).await;
    let constraint = format!("test_disable_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("ALTER TABLE reply_configurations ADD CONSTRAINT {constraint} CHECK (revision <> '{}' OR disabled_at IS NULL)", config.revision)).execute(&pool).await.unwrap();
    let request = reserve(&store, &owner, &conversation, &config).await;
    let sender = Sender::new(3);
    let executor = BudgetedReplyExecutor::new(store.clone(), sender.clone(), config.clone());
    executor.tick().await;
    assert_eq!(charged(&pool, &request).await, 1124);
    let next = reserve(&store, &owner, &conversation, &config).await;
    executor.tick().await;
    assert_eq!(sender.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        store
            .get_reply(&owner.id, &conversation, &next)
            .await
            .unwrap()
            .status,
        ReplyStatus::Queued
    );
    sqlx::query(&format!(
        "ALTER TABLE reply_configurations DROP CONSTRAINT {constraint}"
    ))
    .execute(&pool)
    .await
    .unwrap();
    executor.tick().await;
    assert!(
        store
            .check_reply_configuration(&config, &budget())
            .await
            .is_err()
    );
    assert_eq!(sender.calls.load(Ordering::SeqCst), 1);
    cleanup(&pool, &owner, &config).await;
}
