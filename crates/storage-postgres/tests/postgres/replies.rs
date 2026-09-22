use personal_ai_domain::{User, UserId};
use personal_ai_storage::{
    MetadataStore, StorageError,
    conversations::ConversationStore,
    messages::MessageStore,
    replies::{ReplyConfiguration, ReplyOutcome, ReplyStatus, ReplyStore},
};
use personal_ai_storage_postgres::PostgresStore;
use uuid::Uuid;

fn id() -> String {
    Uuid::new_v4().to_string()
}
fn configuration() -> ReplyConfiguration {
    ReplyConfiguration {
        model: "local-fixture".into(),
        revision: "v1".into(),
    }
}
async fn fixture() -> (PostgresStore, sqlx::PgPool, UserId, String) {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let user = User {
        id: UserId::new(id()),
        email: format!("{}@replies.example", id()),
        display_name: "回复验收".into(),
    };
    store.save_user(&user).await.unwrap();
    let conversation = store
        .create_conversation(&user.id, &id(), "回复")
        .await
        .unwrap()
        .id;
    store
        .append_message(&user.id, &conversation, &id(), "问题")
        .await
        .unwrap();
    (
        store,
        sqlx::PgPool::connect(&url).await.unwrap(),
        user.id,
        conversation,
    )
}
async fn budget(pool: &sqlx::PgPool, owner: &UserId) -> i64 {
    sqlx::query_scalar(
        "SELECT COALESCE(sum(reserved),0)::bigint FROM reply_daily_budgets WHERE user_id=$1",
    )
    .bind(Uuid::parse_str(owner.as_str()).unwrap())
    .fetch_one(pool)
    .await
    .unwrap()
}
async fn cleanup(pool: &sqlx::PgPool, owner: &UserId) {
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn replies_freeze_context_replay_and_isolate_owners() {
    let (store, pool, owner, conversation) = fixture().await;
    let request = id();
    let config = configuration();
    let (a, b) = tokio::join!(
        store.reserve_reply(&owner, &conversation, &request, 1, &config),
        store.reserve_reply(&owner, &conversation, &request, 1, &config)
    );
    let first = a.unwrap();
    assert_eq!(first, b.unwrap());
    assert_eq!(budget(&pool, &owner).await, 1);
    let listed = store.list_replies(&owner, &conversation).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].context.is_none());
    assert_eq!(listed[0].request_id, request);
    assert_eq!(first.context.as_ref().unwrap().user_messages, vec!["问题"]);
    assert_eq!(first.context.as_ref().unwrap().max_output_tokens, 1024);
    store
        .append_message(&owner, &conversation, &id(), "新增内容")
        .await
        .unwrap();
    let changed = ReplyConfiguration {
        model: "new-model".into(),
        revision: "v2".into(),
    };
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .reserve_reply(&owner, &conversation, &request, 1, &changed)
            .await
            .unwrap(),
        first
    );
    assert!(matches!(
        store
            .reserve_reply(&owner, &conversation, &request, 2, &config)
            .await,
        Err(StorageError::Conflict(_))
    ));
    assert!(matches!(
        store
            .reserve_reply(&owner, &conversation, &id(), 1, &config)
            .await,
        Err(StorageError::Conflict(_))
    ));
    assert!(matches!(
        store
            .reserve_reply(&owner, &conversation, &id(), 2, &config)
            .await,
        Err(StorageError::Conflict(_))
    ));
    let (_, foreign_pool, foreign, _) = fixture().await;
    assert_foreign_denied(&store, &foreign, &conversation, &request).await;
    cleanup(&foreign_pool, &foreign).await;
    assert!(matches!(
        store
            .finish_reply(&owner, &conversation, &request, ReplyOutcome::Unknown)
            .await,
        Err(StorageError::Conflict(_))
    ));
    let (a, b) = tokio::join!(
        store.claim_reply(&owner, &conversation, &request),
        store.claim_reply(&owner, &conversation, &request)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let done = store
        .finish_reply(
            &owner,
            &conversation,
            &request,
            ReplyOutcome::Succeeded("回答".into()),
        )
        .await
        .unwrap();
    assert_eq!(done.status, ReplyStatus::Succeeded);
    assert!(done.context.is_none());
    assert_eq!(
        store
            .finish_reply(
                &owner,
                &conversation,
                &request,
                ReplyOutcome::Succeeded("不能覆盖".into())
            )
            .await
            .unwrap(),
        done
    );
    assert_eq!(budget(&pool, &owner).await, 1);
    cleanup(&pool, &owner).await;
}

async fn assert_foreign_denied(
    store: &PostgresStore,
    foreign: &UserId,
    conversation: &str,
    request: &str,
) {
    let config = configuration();
    assert!(matches!(
        store.list_replies(foreign, conversation).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.get_reply(foreign, conversation, request).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store
            .reserve_reply(foreign, conversation, request, 1, &config)
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.claim_reply(foreign, conversation, request).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.cancel_reply(foreign, conversation, request).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store
            .finish_reply(foreign, conversation, request, ReplyOutcome::Unknown)
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.expire_reply(foreign, conversation, request).await,
        Err(StorageError::NotFound)
    ));
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn reply_cancel_claim_races_do_not_refund_dispatched_requests() {
    let (store, pool, owner, conversation) = fixture().await;
    let request = id();
    store
        .reserve_reply(&owner, &conversation, &request, 1, &configuration())
        .await
        .unwrap();
    let cancelled = store
        .cancel_reply(&owner, &conversation, &request)
        .await
        .unwrap();
    assert_eq!(cancelled.status, ReplyStatus::Cancelled);
    assert!(cancelled.context.is_none());
    store
        .cancel_reply(&owner, &conversation, &request)
        .await
        .unwrap();
    assert_eq!(budget(&pool, &owner).await, 0);
    assert!(
        store
            .claim_reply(&owner, &conversation, &request)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .reserve_reply(&owner, &conversation, &request, 1, &configuration())
            .await
            .unwrap(),
        cancelled
    );
    let mut dispatched = 0;
    for _ in 0..8 {
        let request = id();
        store
            .reserve_reply(&owner, &conversation, &request, 1, &configuration())
            .await
            .unwrap();
        let (claim, cancel) = tokio::join!(
            store.claim_reply(&owner, &conversation, &request),
            store.cancel_reply(&owner, &conversation, &request)
        );
        dispatched += i64::from(claim.is_ok());
        assert_eq!(cancel.unwrap().status, ReplyStatus::Cancelled);
        let late = store
            .finish_reply(
                &owner,
                &conversation,
                &request,
                ReplyOutcome::Succeeded("晚到".into()),
            )
            .await
            .unwrap();
        assert_eq!(late.status, ReplyStatus::Cancelled);
        assert!(late.output.is_none());
    }
    assert_eq!(budget(&pool, &owner).await, dispatched);
    cleanup(&pool, &owner).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn expired_replies_never_resend_or_accept_late_output() {
    let (store, pool, owner, conversation) = fixture().await;
    for finish in [false, true] {
        let request = id();
        store
            .reserve_reply(&owner, &conversation, &request, 1, &configuration())
            .await
            .unwrap();
        store
            .claim_reply(&owner, &conversation, &request)
            .await
            .unwrap();
        assert_eq!(
            store
                .expire_reply(&owner, &conversation, &request)
                .await
                .unwrap()
                .status,
            ReplyStatus::Dispatching
        );
        sqlx::query("UPDATE conversation_replies SET dispatched_at=clock_timestamp()-interval '121 seconds' WHERE conversation_id=$1 AND request_id=$2")
            .bind(Uuid::parse_str(&conversation).unwrap()).bind(Uuid::parse_str(&request).unwrap()).execute(&pool).await.unwrap();
        let result = if finish {
            store
                .finish_reply(
                    &owner,
                    &conversation,
                    &request,
                    ReplyOutcome::Succeeded("超时输出".into()),
                )
                .await
        } else {
            store.expire_reply(&owner, &conversation, &request).await
        }
        .unwrap();
        assert_eq!(result.status, ReplyStatus::Unknown);
        assert!(result.output.is_none());
        assert!(result.context.is_none());
        assert!(
            store
                .claim_reply(&owner, &conversation, &request)
                .await
                .is_err()
        );
    }
    assert_eq!(budget(&pool, &owner).await, 2);
    cleanup(&pool, &owner).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn reply_quota_survives_deletion_and_serializes_last_slot() {
    let (store, pool, owner, conversation) = fixture().await;
    let config = configuration();
    for _ in 0..19 {
        let request = id();
        store
            .reserve_reply(&owner, &conversation, &request, 1, &config)
            .await
            .unwrap();
        store
            .claim_reply(&owner, &conversation, &request)
            .await
            .unwrap();
        store
            .finish_reply(&owner, &conversation, &request, ReplyOutcome::Failed)
            .await
            .unwrap();
    }
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
        store.reserve_reply(&owner, &conversation, &a_id, 1, &config),
        store.reserve_reply(&owner, &other, &b_id, 1, &config)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(budget(&pool, &owner).await, 20);
    let (chosen, request) = if a.is_ok() {
        (&conversation, &a_id)
    } else {
        (&other, &b_id)
    };
    store.delete_conversation(&owner, chosen).await.unwrap();
    assert_eq!(budget(&pool, &owner).await, 19);
    assert!(matches!(
        store.get_reply(&owner, chosen, request).await,
        Err(StorageError::NotFound)
    ));
    let fresh = store
        .create_conversation(&owner, &id(), "新对话")
        .await
        .unwrap()
        .id;
    store
        .append_message(&owner, &fresh, &id(), "问题")
        .await
        .unwrap();
    let request = id();
    store
        .reserve_reply(&owner, &fresh, &request, 1, &config)
        .await
        .unwrap();
    store.claim_reply(&owner, &fresh, &request).await.unwrap();
    let (delete, finish) = tokio::join!(
        store.delete_conversation(&owner, &fresh),
        store.finish_reply(
            &owner,
            &fresh,
            &request,
            ReplyOutcome::Succeeded("晚到".into())
        )
    );
    delete.unwrap();
    assert!(finish.is_ok() || matches!(finish, Err(StorageError::NotFound)));
    store.delete_conversation(&owner, &fresh).await.unwrap();
    assert_eq!(budget(&pool, &owner).await, 20);
    let cleared: bool = sqlx::query_scalar("SELECT context IS NULL AND output IS NULL FROM conversation_replies WHERE conversation_id=$1 AND request_id=$2")
        .bind(Uuid::parse_str(&fresh).unwrap()).bind(Uuid::parse_str(&request).unwrap()).fetch_one(&pool).await.unwrap();
    assert!(cleared);
    // 模拟墓碑清理；独立账本不能被级联删除。
    sqlx::query("DELETE FROM conversations WHERE user_id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let fresh = store
        .create_conversation(&owner, &id(), "重建")
        .await
        .unwrap()
        .id;
    store
        .append_message(&owner, &fresh, &id(), "问题")
        .await
        .unwrap();
    assert!(matches!(
        store.reserve_reply(&owner, &fresh, &id(), 1, &config).await,
        Err(StorageError::Conflict(_))
    ));
    assert_eq!(budget(&pool, &owner).await, 20);
    cleanup(&pool, &owner).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn reply_cancel_refunds_original_day_and_bounds_retained_requests() {
    let (store, pool, owner, conversation) = fixture().await;
    let request = id();
    store
        .reserve_reply(&owner, &conversation, &request, 1, &configuration())
        .await
        .unwrap();
    // 不等待真实午夜：把预留与账本一起移动到昨日，模拟跨日取消。
    sqlx::query("UPDATE reply_daily_budgets SET day=day-1 WHERE user_id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE conversation_replies SET budget_day=budget_day-1 WHERE conversation_id=$1")
        .bind(Uuid::parse_str(&conversation).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    store
        .cancel_reply(&owner, &conversation, &request)
        .await
        .unwrap();
    assert_eq!(budget(&pool, &owner).await, 0);
    for _ in 0..99 {
        let request = id();
        store
            .reserve_reply(&owner, &conversation, &request, 1, &configuration())
            .await
            .unwrap();
        store
            .cancel_reply(&owner, &conversation, &request)
            .await
            .unwrap();
    }
    assert!(matches!(
        store
            .reserve_reply(&owner, &conversation, &id(), 1, &configuration())
            .await,
        Err(StorageError::Conflict(_))
    ));
    assert_eq!(budget(&pool, &owner).await, 0);
    cleanup(&pool, &owner).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn invalid_reply_inputs_do_not_reserve_budget() {
    let (store, pool, owner, conversation) = fixture().await;
    let config = configuration();
    for revision in [-1, 0, 2, 101] {
        assert!(
            store
                .reserve_reply(&owner, &conversation, &id(), revision, &config)
                .await
                .is_err()
        );
    }
    assert!(matches!(
        store
            .reserve_reply(&owner, &conversation, "bad-id", 1, &config)
            .await,
        Err(StorageError::InvalidData(_))
    ));
    for invalid in ["", " ", "\0", &"x".repeat(129)] {
        let config = ReplyConfiguration {
            model: invalid.into(),
            revision: "v1".into(),
        };
        assert!(matches!(
            store
                .reserve_reply(&owner, &conversation, &id(), 1, &config)
                .await,
            Err(StorageError::InvalidData(_))
        ));
    }
    let empty = store
        .create_conversation(&owner, &id(), "空对话")
        .await
        .unwrap()
        .id;
    assert!(matches!(
        store.reserve_reply(&owner, &empty, &id(), 0, &config).await,
        Err(StorageError::InvalidData(_))
    ));
    assert_eq!(budget(&pool, &owner).await, 0);
    cleanup(&pool, &owner).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn reply_output_limits_and_dispatched_cancellation_are_enforced() {
    let (store, pool, owner, conversation) = fixture().await;
    let config = configuration();
    let request = id();
    store
        .reserve_reply(&owner, &conversation, &request, 1, &config)
        .await
        .unwrap();
    store
        .claim_reply(&owner, &conversation, &request)
        .await
        .unwrap();
    for invalid in ["", " ", "\0", &"x".repeat(16385)] {
        assert!(matches!(
            store
                .finish_reply(
                    &owner,
                    &conversation,
                    &request,
                    ReplyOutcome::Succeeded(invalid.into())
                )
                .await,
            Err(StorageError::InvalidData(_))
        ));
    }
    assert_eq!(
        store
            .get_reply(&owner, &conversation, &request)
            .await
            .unwrap()
            .status,
        ReplyStatus::Dispatching
    );
    store
        .cancel_reply(&owner, &conversation, &request)
        .await
        .unwrap();
    let late = store
        .finish_reply(
            &owner,
            &conversation,
            &request,
            ReplyOutcome::Succeeded("晚到".into()),
        )
        .await
        .unwrap();
    assert_eq!(late.status, ReplyStatus::Cancelled);
    assert!(late.output.is_none());
    assert_eq!(budget(&pool, &owner).await, 1);
    let request = id();
    store
        .reserve_reply(&owner, &conversation, &request, 1, &config)
        .await
        .unwrap();
    store
        .claim_reply(&owner, &conversation, &request)
        .await
        .unwrap();
    let output = "x".repeat(16384);
    assert_eq!(
        store
            .finish_reply(
                &owner,
                &conversation,
                &request,
                ReplyOutcome::Succeeded(output.clone())
            )
            .await
            .unwrap()
            .output,
        Some(output)
    );
    cleanup(&pool, &owner).await;
}
