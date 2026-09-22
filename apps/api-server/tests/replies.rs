use api_server::ReplyRuntime;
use personal_ai_domain::{User, UserId};
use personal_ai_storage::{
    MetadataStore,
    conversations::ConversationStore,
    messages::MessageStore,
    replies::{ReplyConfiguration, ReplyStatus, ReplyStore},
};
use personal_ai_storage_postgres::PostgresStore;
use std::sync::Arc;
use uuid::Uuid;

async fn fixture() -> (String, Arc<PostgresStore>, sqlx::PgPool, User, String) {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = Arc::new(PostgresStore::connect(&url).await.unwrap());
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let owner = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@reply-worker.example", Uuid::new_v4()),
        display_name: "执行器验收".into(),
    };
    store.save_user(&owner).await.unwrap();
    let conversation = store
        .create_conversation(&owner.id, &Uuid::new_v4().to_string(), "回复")
        .await
        .unwrap()
        .id;
    store
        .append_message(
            &owner.id,
            &conversation,
            &Uuid::new_v4().to_string(),
            "上下文",
        )
        .await
        .unwrap();
    (url, store, pool, owner, conversation)
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn fixture_worker_recovers_queue_and_expires_without_redispatch() {
    let (url, store, pool, owner, conversation) = fixture().await;
    let config = ReplyConfiguration {
        model: "builtin-fixture".into(),
        revision: "fixture-v1".into(),
    };
    let request = Uuid::new_v4().to_string();
    store
        .reserve_reply(&owner.id, &conversation, &request, 1, &config)
        .await
        .unwrap();
    let disabled = ReplyRuntime::new(store.clone(), None).unwrap();
    disabled.tick().await;
    assert_eq!(
        store
            .get_reply(&owner.id, &conversation, &request)
            .await
            .unwrap()
            .status,
        ReplyStatus::Queued
    );
    // 新建进程等价的连接和执行器，从持久化队列恢复；两个实例也只能领取一次。
    let reopened = Arc::new(PostgresStore::connect(&url).await.unwrap());
    let first = ReplyRuntime::new(store.clone(), Some("fixture")).unwrap();
    let second = ReplyRuntime::new(reopened, Some("fixture")).unwrap();
    tokio::join!(first.tick(), second.tick());
    let done = store
        .get_reply(&owner.id, &conversation, &request)
        .await
        .unwrap();
    assert_eq!(done.status, ReplyStatus::Succeeded);
    assert_eq!(
        done.output.as_deref(),
        Some("本地测试回复（非模型生成）：上下文")
    );
    first.tick().await;
    assert_eq!(
        store
            .get_reply(&owner.id, &conversation, &request)
            .await
            .unwrap(),
        done
    );
    let request = Uuid::new_v4().to_string();
    store
        .reserve_reply(&owner.id, &conversation, &request, 1, &config)
        .await
        .unwrap();
    store
        .claim_reply(&owner.id, &conversation, &request)
        .await
        .unwrap();
    sqlx::query("UPDATE conversation_replies SET dispatched_at=clock_timestamp()-interval '121 seconds' WHERE conversation_id=$1 AND request_id=$2")
        .bind(Uuid::parse_str(&conversation).unwrap()).bind(Uuid::parse_str(&request).unwrap()).execute(&pool).await.unwrap();
    disabled.tick().await;
    let unknown = store
        .get_reply(&owner.id, &conversation, &request)
        .await
        .unwrap();
    assert_eq!(unknown.status, ReplyStatus::Unknown);
    assert!(unknown.output.is_none());
    first.tick().await;
    assert_eq!(
        store
            .get_reply(&owner.id, &conversation, &request)
            .await
            .unwrap(),
        unknown
    );
    let request = Uuid::new_v4().to_string();
    let unsupported = ReplyConfiguration {
        model: "external".into(),
        revision: "v1".into(),
    };
    store
        .reserve_reply(&owner.id, &conversation, &request, 1, &unsupported)
        .await
        .unwrap();
    first.tick().await;
    assert_eq!(
        store
            .get_reply(&owner.id, &conversation, &request)
            .await
            .unwrap()
            .status,
        ReplyStatus::Failed
    );
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.id.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
}
