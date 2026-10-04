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
#[ignore = "需要一次性 TEST_REDIS_URL"]
async fn message_cache_rejects_stale_snapshots_and_preserves_deletion_fences() {
    use personal_ai_storage::messages::{Message, MessageCache, MessageSnapshot};
    use personal_ai_storage_redis::RedisMessageCache;
    let url = std::env::var("TEST_REDIS_URL").unwrap();
    let cache = RedisMessageCache::new(&url).unwrap();
    let owner = UserId::new(uuid::Uuid::new_v4().to_string());
    let conversation = uuid::Uuid::new_v4().to_string();
    let first = MessageSnapshot {
        revision: 1,
        deleted: false,
        messages: vec![Message {
            id: uuid::Uuid::new_v4().to_string(),
            sequence: 1,
            content: "消息".into(),
            created_at_unix_ms: 1,
        }],
    };
    cache.put(&owner, &conversation, &first).await.unwrap();
    assert_eq!(
        cache.get(&owner, &conversation).await.unwrap(),
        Some(first.clone())
    );
    assert!(
        cache
            .get(&UserId::new("other"), &conversation)
            .await
            .unwrap()
            .is_none()
    );
    let deleted = MessageSnapshot {
        revision: 2,
        deleted: true,
        messages: vec![],
    };
    cache.put(&owner, &conversation, &deleted).await.unwrap();
    cache.put(&owner, &conversation, &first).await.unwrap();
    let reopened = RedisMessageCache::new(&url).unwrap();
    assert_eq!(
        reopened.get(&owner, &conversation).await.unwrap(),
        Some(deleted)
    );
    // 仅剩无正文的短期删除栅栏；一次性 Redis 由验收环境负责销毁。
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
    assert_eq!(
        store.recent(&other, &conversation, 3).await.unwrap(),
        [] as [personal_ai_storage::MemoryEntry; 0]
    );
    assert_eq!(
        store
            .recent(&owner, &ConversationId::new("two"), 3)
            .await
            .unwrap(),
        [] as [personal_ai_storage::MemoryEntry; 0]
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
    assert_ne!(
        store.recent(&owner, &conversation, 3).await.unwrap(),
        [] as [personal_ai_storage::MemoryEntry; 0]
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(
        store.recent(&owner, &conversation, 3).await.unwrap(),
        [] as [personal_ai_storage::MemoryEntry; 0]
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

#[tokio::test]
#[ignore = "需要一次性 TEST_REDIS_URL；只使用随机 Pub/Sub 频道"]
async fn review_text_is_live_only_private_bounded_and_never_replayed() {
    use personal_ai_storage::learning::review_text::{ReviewTextBridge, TextKind, TextPacket};
    use personal_ai_storage_redis::RedisReviewText;
    let url = std::env::var("TEST_REDIS_URL").unwrap();
    let publisher_bridge = RedisReviewText::new(&url).unwrap();
    let observer_bridge = RedisReviewText::new(&url).unwrap();
    let owner = UserId::new(uuid::Uuid::new_v4().to_string());
    let request = uuid::Uuid::new_v4().to_string();
    let mut publisher = publisher_bridge.publisher(&owner, &request).await.unwrap();
    let packet = |sequence, text: &str| TextPacket {
        sequence,
        detail: TextKind::Delta { text: text.into() },
    };
    publisher
        .publish(packet(0, "before observation"))
        .await
        .unwrap();
    let mut observer = observer_bridge.subscribe(&owner, &request).await.unwrap();
    let mut other = observer_bridge
        .subscribe(&UserId::new("other"), &request)
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), observer.next())
            .await
            .is_err()
    );
    publisher
        .publish(packet(1, "临时正文 <script>"))
        .await
        .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(2), observer.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(received.sequence, 1);
    assert!(matches!(received.detail, TextKind::Delta { text } if text == "临时正文 <script>"));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), other.next())
            .await
            .is_err()
    );
    publisher
        .publish(TextPacket {
            sequence: 2,
            detail: TextKind::Clear,
        })
        .await
        .unwrap();
    publisher
        .publish(TextPacket {
            sequence: 3,
            detail: TextKind::End,
        })
        .await
        .unwrap();
    assert!(matches!(
        observer.next().await.unwrap().unwrap().detail,
        TextKind::Clear
    ));
    assert!(matches!(
        observer.next().await.unwrap().unwrap().detail,
        TextKind::End
    ));
    assert!(observer.next().await.unwrap().is_none());
    assert!(publisher.publish(packet(4, "late")).await.is_err());
    let mut reopened = observer_bridge.subscribe(&owner, &request).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), reopened.next())
            .await
            .is_err()
    );
    // Pub/Sub creates no body keys, so persistence settings cannot retain these packets.
    let client = redis::Client::open(url).unwrap();
    let mut connection = client.get_multiplexed_async_connection().await.unwrap();
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg("learning-text:*")
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(keys, [] as [String; 0]);
}

#[tokio::test]
#[ignore = "需要一次性 TEST_REDIS_URL"]
async fn review_text_slow_reader_closes_and_releases_its_subscription() {
    use personal_ai_storage::learning::review_text::{ReviewTextBridge, TextKind, TextPacket};
    use personal_ai_storage_redis::RedisReviewText;
    let bridge = RedisReviewText::new(&std::env::var("TEST_REDIS_URL").unwrap()).unwrap();
    let owner = UserId::new(uuid::Uuid::new_v4().to_string());
    let request = uuid::Uuid::new_v4().to_string();
    let mut observer = bridge.subscribe(&owner, &request).await.unwrap();
    let mut publisher = bridge.publisher(&owner, &request).await.unwrap();
    for sequence in 0..40 {
        publisher
            .publish(TextPacket {
                sequence,
                detail: TextKind::Delta {
                    text: "private".into(),
                },
            })
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(observer.next().await.is_err());
    drop(observer);
    let mut fresh = bridge.subscribe(&owner, &request).await.unwrap();
    publisher
        .publish(TextPacket {
            sequence: 40,
            detail: TextKind::End,
        })
        .await
        .unwrap();
    assert!(matches!(
        fresh.next().await.unwrap().unwrap().detail,
        TextKind::End
    ));
}
