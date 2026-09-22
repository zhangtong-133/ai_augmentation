use personal_ai_domain::{ConversationId, UserId};
use personal_ai_storage::{MemoryEntry, MemoryStore, StorageError};
use personal_ai_storage_redis::RedisMemoryStore;
use std::time::Duration;

fn entry(value: &str) -> MemoryEntry {
    MemoryEntry {
        key: "user".into(),
        value: value.into(),
        created_at_unix_ms: 123,
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_REDIS_URL；仅写入随机用户命名空间"]
async fn isolation_capacity_and_expiration() {
    let url = std::env::var("TEST_REDIS_URL").expect("TEST_REDIS_URL required");
    let store = RedisMemoryStore::new(&url, 2, 3).unwrap();
    let owner = UserId::new(uuid::Uuid::new_v4().to_string());
    let other = UserId::new(uuid::Uuid::new_v4().to_string());
    let conversation = ConversationId::new("one");
    for value in ["1", "2", "3", "4"] {
        store
            .append(&owner, &conversation, &entry(value))
            .await
            .unwrap();
    }
    assert_eq!(
        store.recent(&owner, &conversation, 2).await.unwrap(),
        vec![entry("3"), entry("4")]
    );
    assert!(
        store
            .recent(&other, &conversation, 3)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .recent(&owner, &ConversationId::new("two"), 3)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.recent(&owner, &conversation, 4).await.is_err());
    assert!(store.recent(&owner, &conversation, 0).await.is_err());
    assert!(
        store
            .append(&owner, &conversation, &entry(&"x".repeat(4097)))
            .await
            .is_err()
    );
    let reopened = RedisMemoryStore::new(&url, 2, 3).unwrap();
    assert_eq!(
        reopened
            .recent(&owner, &conversation, 3)
            .await
            .unwrap()
            .len(),
        3
    );
    // 读取不得续期；两次读取跨越最初的写入 TTL。
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(
        !store
            .recent(&owner, &conversation, 3)
            .await
            .unwrap()
            .is_empty()
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(
        store
            .recent(&owner, &conversation, 3)
            .await
            .unwrap()
            .is_empty()
    );

    // 过期后可以重新创建；写入会为整个窗口续期。
    store
        .append(&owner, &conversation, &entry("续期前"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    store
        .append(&owner, &conversation, &entry("续期后"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(
        store.recent(&owner, &conversation, 3).await.unwrap().len(),
        2
    );
    store.clear(&other, &conversation).await.unwrap();
    assert_eq!(
        store.recent(&owner, &conversation, 3).await.unwrap().len(),
        2
    );
    store.clear(&owner, &conversation).await.unwrap();
}

#[tokio::test]
#[ignore = "需要一次性 TEST_REDIS_URL；仅写入随机用户命名空间"]
async fn concurrent_quota_and_clear() {
    let url = std::env::var("TEST_REDIS_URL").expect("TEST_REDIS_URL required");
    let owner = UserId::new(uuid::Uuid::new_v4().to_string());
    let conversation = ConversationId::new("one");
    let store = RedisMemoryStore::new(&url, 60, 3).unwrap();
    let mut jobs = tokio::task::JoinSet::new();
    for index in 0..40 {
        let store = store.clone();
        let owner = owner.clone();
        jobs.spawn(async move {
            store
                .append(
                    &owner,
                    &ConversationId::new(index.to_string()),
                    &entry("并发"),
                )
                .await
        });
    }
    let mut accepted = 0;
    while let Some(result) = jobs.join_next().await {
        match result.unwrap() {
            Ok(()) => accepted += 1,
            Err(StorageError::Conflict(_)) => (),
            error => panic!("unexpected result: {error:?}"),
        }
    }
    assert_eq!(accepted, 32);
    // 清理仅针对本测试的随机用户；清空可重复调用且释放名额。
    for index in 0..40 {
        store
            .clear(&owner, &ConversationId::new(index.to_string()))
            .await
            .unwrap();
    }
    store.clear(&owner, &conversation).await.unwrap();
    store.clear(&owner, &conversation).await.unwrap();
    store
        .append(&owner, &conversation, &entry("新对话"))
        .await
        .unwrap();
    let mut jobs = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let store = store.clone();
        let owner = owner.clone();
        let conversation = conversation.clone();
        jobs.spawn(async move {
            store
                .append(&owner, &conversation, &entry("追加"))
                .await
                .unwrap();
        });
    }
    while let Some(result) = jobs.join_next().await {
        result.unwrap();
    }
    assert_eq!(
        store.recent(&owner, &conversation, 3).await.unwrap().len(),
        3
    );
    store.clear(&owner, &conversation).await.unwrap();
}

#[tokio::test]
#[ignore = "需要一次性 TEST_REDIS_URL；仅写入随机用户命名空间"]
async fn expired_slots_are_reclaimed_without_losing_longer_lived_slots() {
    let url = std::env::var("TEST_REDIS_URL").expect("TEST_REDIS_URL required");
    let owner = UserId::new(uuid::Uuid::new_v4().to_string());
    let short = RedisMemoryStore::new(&url, 1, 3).unwrap();
    let long = RedisMemoryStore::new(&url, 60, 3).unwrap();
    for index in 0..31 {
        long.append(
            &owner,
            &ConversationId::new(index.to_string()),
            &entry("长窗口"),
        )
        .await
        .unwrap();
    }
    short
        .append(&owner, &ConversationId::new("short"), &entry("短窗口"))
        .await
        .unwrap();
    assert!(matches!(
        long.append(&owner, &ConversationId::new("new"), &entry("超额"))
            .await,
        Err(StorageError::Conflict(_))
    ));
    tokio::time::sleep(Duration::from_millis(1200)).await;
    long.append(&owner, &ConversationId::new("new"), &entry("回收名额"))
        .await
        .unwrap();
    assert!(matches!(
        long.append(&owner, &ConversationId::new("overflow"), &entry("仍超额"))
            .await,
        Err(StorageError::Conflict(_))
    ));
    for index in 0..31 {
        long.clear(&owner, &ConversationId::new(index.to_string()))
            .await
            .unwrap();
    }
    long.clear(&owner, &ConversationId::new("new"))
        .await
        .unwrap();
}

#[tokio::test]
async fn unresponsive_service_is_an_error() {
    let owner = UserId::new("test");
    let conversation = ConversationId::new("test");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unavailable = RedisMemoryStore::new(
        &format!("redis://{}/", listener.local_addr().unwrap()),
        60,
        3,
    )
    .unwrap();
    // 端口接受连接但不响应，证明超时和故障不会伪装成空结果。
    assert!(matches!(
        unavailable.recent(&owner, &conversation, 3).await,
        Err(StorageError::Unavailable(_))
    ));
}
