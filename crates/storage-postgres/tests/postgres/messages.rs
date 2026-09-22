use personal_ai_domain::{User, UserId};
use personal_ai_storage::{
    MetadataStore, StorageError, conversations::ConversationStore, messages::MessageStore,
};
use personal_ai_storage_postgres::PostgresStore;
use uuid::Uuid;

async fn fixture() -> (PostgresStore, sqlx::PgPool, UserId, String) {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let user = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@messages.example", Uuid::new_v4()),
        display_name: "消息验收".into(),
    };
    store.save_user(&user).await.unwrap();
    let conversation = store
        .create_conversation(&user.id, &Uuid::new_v4().to_string(), "消息")
        .await
        .unwrap();
    (
        store,
        sqlx::PgPool::connect(&url).await.unwrap(),
        user.id,
        conversation.id,
    )
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
async fn messages_are_idempotent_owner_scoped_ordered_and_bounded() {
    let (store, pool, owner, conversation) = fixture().await;
    let request = Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        store.append_message(&owner, &conversation, &request, "你好"),
        store.append_message(&owner, &conversation, &request, "你好")
    );
    let first = a.unwrap();
    assert_eq!(first, b.unwrap());
    assert_eq!(first.sequence, 1);
    assert!(matches!(
        store
            .append_message(&owner, &conversation, &request, "不同")
            .await,
        Err(StorageError::Conflict(_))
    ));
    let foreign = UserId::new(Uuid::new_v4().to_string());
    assert!(matches!(
        store
            .append_message(&foreign, &conversation, &request, "你好")
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.message_snapshot(&foreign, &conversation).await,
        Err(StorageError::NotFound)
    ));
    for _ in 1..99 {
        store
            .append_message(&owner, &conversation, &Uuid::new_v4().to_string(), "消息")
            .await
            .unwrap();
    }
    let key_a = Uuid::new_v4().to_string();
    let key_b = Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        store.append_message(&owner, &conversation, &key_a, "最后"),
        store.append_message(&owner, &conversation, &key_b, "最后")
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(
        a.as_ref().err().or(b.as_ref().err()),
        Some(StorageError::Conflict(_))
    ));
    assert_eq!(
        store
            .append_message(&owner, &conversation, &request, "你好")
            .await
            .unwrap(),
        first
    );
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let snapshot = reopened
        .message_snapshot(&owner, &conversation)
        .await
        .unwrap();
    assert_eq!(snapshot.revision, 100);
    assert_eq!(snapshot.messages.len(), 100);
    assert_eq!(
        snapshot
            .messages
            .iter()
            .map(|m| m.sequence)
            .collect::<Vec<_>>(),
        (1..=100).collect::<Vec<_>>()
    );
    cleanup(&pool, &owner).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn deletion_serializes_with_append_and_cleanup_survives_reconnection() {
    let (store, pool, owner, conversation) = fixture().await;
    let request = Uuid::new_v4().to_string();
    let (append, deleted) = tokio::join!(
        store.append_message(&owner, &conversation, &request, "竞态"),
        store.delete_conversation(&owner, &conversation)
    );
    deleted.unwrap();
    assert!(append.is_ok() || matches!(append, Err(StorageError::NotFound)));
    assert!(matches!(
        store
            .append_message(&owner, &conversation, &request, "竞态")
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.message_revision(&owner, &conversation).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.message_snapshot(&owner, &conversation).await,
        Err(StorageError::NotFound)
    ));
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM conversation_messages WHERE conversation_id=$1")
            .bind(Uuid::parse_str(&conversation).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0);
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    // 其他并行测试可能占据清理批次前 20 条，精确验证本测试的持久化待办。
    let revision = sqlx::query_scalar(
        "SELECT message_revision FROM conversations WHERE id=$1 AND cache_delete_pending",
    )
    .bind(Uuid::parse_str(&conversation).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    let item = personal_ai_storage::messages::CacheDeletion {
        owner: owner.clone(),
        conversation: conversation.clone(),
        revision,
    };
    reopened.acknowledge_cache_deletion(&item).await.unwrap();
    assert!(
        !reopened
            .pending_cache_deletions()
            .await
            .unwrap()
            .iter()
            .any(|item| item.conversation == conversation)
    );
    cleanup(&pool, &owner).await;
}
