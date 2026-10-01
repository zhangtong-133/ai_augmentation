use super::*;
use personal_ai_storage::tool_calls::{
    NewToolCall, ToolCallFinish, ToolCallOutcome, ToolCallStart, ToolCallStore,
};
use std::sync::Arc;

struct Fixture {
    store: Arc<PostgresStore>,
    pool: sqlx::PgPool,
    owner: UserId,
    foreign: UserId,
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
                email: format!("{}@tool-calls.example", Uuid::new_v4()),
                display_name: "工具审计".into(),
            };
            store.save_user(&user).await.unwrap();
            owners.push(user.id);
        }
        Self {
            store,
            pool,
            owner: owners.remove(0),
            foreign: owners.remove(0),
        }
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

fn input() -> NewToolCall {
    NewToolCall {
        request_id: Uuid::new_v4().to_string(),
        tool: "knowledge_search".into(),
        arguments_digest: "a".repeat(64),
        input_bytes: 2,
    }
}
fn finish(outcome: ToolCallOutcome) -> ToolCallFinish {
    ToolCallFinish {
        outcome,
        output_bytes: (outcome == ToolCallOutcome::Succeeded).then_some(42),
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn tool_call_ids_are_concurrent_durable_owner_scoped_and_conflicts_never_spend_twice() {
    let f = Fixture::new().await;
    let input = input();
    let (a, b) = tokio::join!(
        f.store.start_tool_call(&f.owner, &input),
        f.store.start_tool_call(&f.owner, &input)
    );
    let outcomes = [a.unwrap(), b.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|v| matches!(v, ToolCallStart::Started(_)))
            .count(),
        1
    );
    let audit = f.store.audit_tool_calls(&f.owner, None).await.unwrap();
    assert_eq!(audit.used, 1);
    assert_eq!(audit.items.len(), 1);
    assert_eq!(audit.limit, 100);
    let original = audit.items[0].clone();
    let mut conflict = input.clone();
    conflict.arguments_digest = "b".repeat(64);
    assert!(matches!(
        f.store.start_tool_call(&f.owner, &conflict).await,
        Err(StorageError::Conflict(_))
    ));
    conflict = input.clone();
    conflict.tool = "another_tool".into();
    assert!(matches!(
        f.store.start_tool_call(&f.owner, &conflict).await,
        Err(StorageError::Conflict(_))
    ));
    assert!(matches!(
        f.store.get_tool_call(&f.foreign, &input.request_id).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store
            .finish_tool_call(
                &f.foreign,
                &input.request_id,
                finish(ToolCallOutcome::Failed)
            )
            .await,
        Err(StorageError::NotFound)
    ));
    assert_eq!(
        f.store
            .audit_tool_calls(&f.foreign, None)
            .await
            .unwrap()
            .items,
        [] as [personal_ai_storage::tool_calls::ToolCall; 0]
    );
    let reopened = PostgresStore::connect_existing(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .get_tool_call(&f.owner, &input.request_id)
            .await
            .unwrap(),
        original
    );
    assert!(matches!(
        reopened.start_tool_call(&f.owner, &input).await.unwrap(),
        ToolCallStart::Existing(_)
    ));
    assert!(matches!(
        f.store.start_tool_call(&f.foreign, &input).await.unwrap(),
        ToolCallStart::Started(_)
    ));
    assert_eq!(
        f.store.audit_tool_calls(&f.owner, None).await.unwrap().used,
        1
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn tool_daily_limit_is_atomic_and_failed_or_unknown_attempts_do_not_refund() {
    let f = Fixture::new().await;
    let first = input();
    f.store.start_tool_call(&f.owner, &first).await.unwrap();
    f.store
        .finish_tool_call(&f.owner, &first.request_id, finish(ToolCallOutcome::Failed))
        .await
        .unwrap();
    for _ in 0..98 {
        f.store.start_tool_call(&f.owner, &input()).await.unwrap();
    }
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let store = f.store.clone();
        let owner = f.owner.clone();
        tasks.push(tokio::spawn(async move {
            store.start_tool_call(&owner, &input()).await
        }));
    }
    let mut started = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(ToolCallStart::Started(_)) => started += 1,
            Err(StorageError::Conflict(_)) => {}
            other => panic!("unexpected result: {other:?}"),
        }
    }
    assert_eq!(started, 1);
    let audit = f.store.audit_tool_calls(&f.owner, None).await.unwrap();
    assert_eq!(audit.used, 100);
    assert_eq!(audit.items.len(), 100);
    assert!(matches!(
        f.store.start_tool_call(&f.owner, &first).await.unwrap(),
        ToolCallStart::Existing(_)
    ));
    assert!(matches!(
        f.store.start_tool_call(&f.owner, &input()).await,
        Err(StorageError::Conflict(_))
    ));
    assert_eq!(
        f.store
            .audit_tool_calls(&f.owner, Some("2000-01-01"))
            .await
            .unwrap()
            .items,
        [] as [personal_ai_storage::tool_calls::ToolCall; 0]
    );
    assert!(matches!(
        f.store.audit_tool_calls(&f.owner, Some("2026-02-29")).await,
        Err(StorageError::InvalidData(_))
    ));
    let pool = f.pool.clone();
    let owner = Uuid::parse_str(f.owner.as_str()).unwrap();
    f.cleanup().await;
    for table in ["tool_calls", "tool_daily_budgets"] {
        let count: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE user_id=$1"))
                .bind(owner)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn tool_outcomes_are_idempotent_and_expired_calls_remain_unknown_without_retry() {
    let f = Fixture::new().await;
    let pending = input();
    f.store.start_tool_call(&f.owner, &pending).await.unwrap();
    sqlx::query("UPDATE tool_calls SET deadline=clock_timestamp()-interval '1 second' WHERE user_id=$1 AND request_id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&pending.request_id).unwrap())
        .execute(&f.pool).await.unwrap();
    assert_eq!(
        f.store
            .get_tool_call(&f.owner, &pending.request_id)
            .await
            .unwrap()
            .status,
        "unknown"
    );
    assert_eq!(
        f.store
            .audit_tool_calls(&f.owner, None)
            .await
            .unwrap()
            .items[0]
            .status,
        "unknown"
    );
    assert!(matches!(
        f.store.start_tool_call(&f.owner, &pending).await.unwrap(),
        ToolCallStart::Existing(_)
    ));
    for outcome in [
        ToolCallOutcome::Succeeded,
        ToolCallOutcome::Failed,
        ToolCallOutcome::TimedOut,
        ToolCallOutcome::Busy,
        ToolCallOutcome::Denied,
        ToolCallOutcome::OutputRejected,
    ] {
        let input = input();
        f.store.start_tool_call(&f.owner, &input).await.unwrap();
        let first = f
            .store
            .finish_tool_call(&f.owner, &input.request_id, finish(outcome))
            .await
            .unwrap();
        assert_eq!(first.status, outcome.as_str());
        assert_eq!(
            f.store
                .finish_tool_call(&f.owner, &input.request_id, finish(outcome))
                .await
                .unwrap(),
            first
        );
        let alternate = if outcome == ToolCallOutcome::Failed {
            ToolCallOutcome::Succeeded
        } else {
            ToolCallOutcome::Failed
        };
        assert!(matches!(
            f.store
                .finish_tool_call(&f.owner, &input.request_id, finish(alternate))
                .await,
            Err(StorageError::Conflict(_))
        ));
    }
    // 结果未知后收到可靠终态可补齐记录；次数不会再次占用或退还。
    f.store
        .finish_tool_call(
            &f.owner,
            &pending.request_id,
            finish(ToolCallOutcome::TimedOut),
        )
        .await
        .unwrap();
    let audit = f.store.audit_tool_calls(&f.owner, None).await.unwrap();
    assert_eq!(audit.used, 7);
    assert!(audit.items.iter().all(|c| c.finished_at_unix_ms.is_some()));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn tool_audits_use_consistent_snapshots_and_invalid_requests_leave_no_budget() {
    let f = Fixture::new().await;
    for invalid in [
        NewToolCall {
            input_bytes: 8193,
            ..input()
        },
        NewToolCall {
            arguments_digest: "secret".into(),
            ..input()
        },
        NewToolCall {
            request_id: "invalid".into(),
            ..input()
        },
        NewToolCall {
            tool: "shell; rm".into(),
            ..input()
        },
    ] {
        assert!(matches!(
            f.store.start_tool_call(&f.owner, &invalid).await,
            Err(StorageError::InvalidData(_))
        ));
    }
    assert_eq!(
        f.store.audit_tool_calls(&f.owner, None).await.unwrap().used,
        0
    );
    for _ in 0..12 {
        let next = input();
        let (started, snapshot) = tokio::join!(
            f.store.start_tool_call(&f.owner, &next),
            f.store.audit_tool_calls(&f.owner, None)
        );
        started.unwrap();
        let audit = snapshot.unwrap();
        assert_eq!(usize::try_from(audit.used).unwrap(), audit.items.len());
    }
    let record = input();
    f.store.start_tool_call(&f.owner, &record).await.unwrap();
    let invalid = ToolCallFinish {
        outcome: ToolCallOutcome::Succeeded,
        output_bytes: Some(65537),
    };
    assert!(matches!(
        f.store
            .finish_tool_call(&f.owner, &record.request_id, invalid)
            .await,
        Err(StorageError::InvalidData(_))
    ));
    assert_eq!(
        f.store
            .get_tool_call(&f.owner, &record.request_id)
            .await
            .unwrap()
            .status,
        "running"
    );
    f.cleanup().await;
}
