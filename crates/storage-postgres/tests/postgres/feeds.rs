use super::*;
use personal_ai_storage::feeds::*;

struct Fixture {
    store: PostgresStore,
    pool: sqlx::PgPool,
    owner: UserId,
    other: UserId,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let store = PostgresStore::connect(&url).await.unwrap();
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let owner = UserId::new(Uuid::new_v4().to_string());
        let other = UserId::new(Uuid::new_v4().to_string());
        for id in [&owner, &other] {
            store
                .save_user(&User {
                    id: id.clone(),
                    email: format!("{}@feeds.example", id.as_str()),
                    display_name: "RSS 测试".into(),
                })
                .await
                .unwrap();
        }
        Self {
            store,
            pool,
            owner,
            other,
        }
    }
    async fn sub(&self) -> Subscription {
        self.store
            .create_subscription(&self.owner, &Uuid::new_v4().to_string(), &input())
            .await
            .unwrap()
    }
    async fn preview(&self, sub: &Subscription) -> Collection {
        self.store
            .preview_collection(
                &self.owner,
                &sub.snapshot.subscription_id,
                &Uuid::new_v4().to_string(),
            )
            .await
            .unwrap()
    }
    async fn claim(&self, sub: &Subscription) -> CollectionClaim {
        let draft = self.preview(sub).await;
        self.store
            .claim_collection(&self.owner, &draft.plan.request_id, &draft.digest)
            .await
            .unwrap()
    }
    async fn cleanup(&self) {
        sqlx::query("DELETE FROM users WHERE id=$1 OR id=$2")
            .bind(Uuid::parse_str(self.owner.as_str()).unwrap())
            .bind(Uuid::parse_str(self.other.as_str()).unwrap())
            .execute(&self.pool)
            .await
            .unwrap();
    }
}
fn input() -> SubscriptionInput {
    SubscriptionInput {
        name: "新闻".into(),
        source_url: "https://EXAMPLE.com:443/rss?private=yes#x".into(),
        enabled: true,
    }
}
fn response(items: &str) -> CollectionOutcome {
    CollectionOutcome::Response(format!("<rss version=\"2.0\"><channel><title>News</title><description>Summary</description><link>https://example.com/</link>{items}</channel></rss>").into_bytes())
}
const ITEM: &str = "<item><guid>stable</guid><title>Original</title></item>";
fn is_conflict<T>(result: Result<T, StorageError>) {
    assert!(
        result
            .err()
            .is_some_and(|error| matches!(error, StorageError::Conflict(_)))
    );
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn subscriptions_and_previews_are_private_versioned_and_idempotent() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let id = &sub.snapshot.subscription_id;
    assert_eq!(
        sub.snapshot.source_url,
        "https://example.com/rss?private=yes"
    );
    assert!(
        f.store
            .create_subscription(&f.owner, id, &input())
            .await
            .unwrap()
            == sub
    );
    assert!(matches!(
        f.store.get_subscription(&f.other, id).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store.update_subscription(&f.other, id, 1, &input()).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store.delete_subscription(&f.other, id, 1).await,
        Err(StorageError::NotFound)
    ));
    assert!(
        f.store
            .list_subscriptions(&f.other, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let draft = f.preview(&sub).await;
    let request = &draft.plan.request_id;
    assert!(
        f.store
            .preview_collection(&f.owner, id, request)
            .await
            .unwrap()
            == draft
    );
    let other_sub = f.sub().await;
    is_conflict(
        f.store
            .preview_collection(&f.owner, &other_sub.snapshot.subscription_id, request)
            .await,
    );
    assert!(matches!(
        f.store.get_collection(&f.other, request).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store
            .claim_collection(&f.other, request, &draft.digest)
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store.cancel_collection(&f.other, request).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store.collection_audit(&f.other, request).await,
        Err(StorageError::NotFound)
    ));
    is_conflict(f.store.claim_collection(&f.owner, request, "wrong").await);
    let mut changed = input();
    changed.enabled = false;
    let disabled = f
        .store
        .update_subscription(&f.owner, id, 1, &changed)
        .await
        .unwrap();
    assert_eq!(disabled.snapshot.revision, 2);
    is_conflict(
        f.store
            .claim_collection(&f.owner, request, &draft.digest)
            .await,
    );
    is_conflict(f.store.update_subscription(&f.owner, id, 1, &input()).await);
    let enabled = f
        .store
        .update_subscription(&f.owner, id, 2, &input())
        .await
        .unwrap();
    assert_eq!(enabled.snapshot.revision, 3);
    is_conflict(
        f.store
            .claim_collection(&f.owner, request, &draft.digest)
            .await,
    );
    assert!(
        f.store
            .preview_collection(&f.owner, id, request)
            .await
            .unwrap()
            == draft
    );
    let cancelled = f.store.cancel_collection(&f.owner, request).await.unwrap();
    assert_eq!(cancelled.status, CollectionStatus::Cancelled);
    assert!(f.store.cancel_collection(&f.owner, request).await.unwrap() == cancelled);
    is_conflict(
        f.store
            .claim_collection(&f.owner, request, &draft.digest)
            .await,
    );
    assert_eq!(
        f.store
            .collection_audit(&f.owner, request)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        f.store
            .list_collections(&f.other, None)
            .await
            .unwrap()
            .items
            .len(),
        0
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn claims_are_single_use_and_results_upsert_atomically_with_audit() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let draft = f.preview(&sub).await;
    let request = &draft.plan.request_id;
    let (a, b) = tokio::join!(
        f.store.claim_collection(&f.owner, request, &draft.digest),
        f.store.claim_collection(&f.owner, request, &draft.digest)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let claim = a.ok().or_else(|| b.ok()).unwrap();
    let another = f.preview(&sub).await;
    is_conflict(
        f.store
            .claim_collection(&f.owner, &another.plan.request_id, &another.digest)
            .await,
    );
    is_conflict(f.store.cancel_collection(&f.owner, request).await);
    let mut bad = claim.clone();
    bad.claim_id = Uuid::new_v4().to_string();
    is_conflict(f.store.finish_collection(&bad, response(ITEM)).await);
    let mut foreign = claim.clone();
    foreign.owner = f.other.clone();
    assert!(matches!(
        f.store.finish_collection(&foreign, response(ITEM)).await,
        Err(StorageError::NotFound)
    ));
    let result = f
        .store
        .finish_collection(&claim, response(&ITEM.repeat(2)))
        .await
        .unwrap();
    assert_eq!(result.status, CollectionStatus::Succeeded);
    assert_eq!(result.counts.inserted, 1);
    assert!(
        f.store
            .finish_collection(&claim, response("malformed"))
            .await
            .unwrap()
            == result
    );
    is_conflict(
        f.store
            .claim_collection(&f.owner, request, &draft.digest)
            .await,
    );
    let entries = f
        .store
        .list_feed_entries(&f.owner, &sub.snapshot.subscription_id, None)
        .await
        .unwrap()
        .items;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].title, "Original");
    assert!(matches!(
        f.store
            .list_feed_entries(&f.other, &sub.snapshot.subscription_id, None)
            .await,
        Err(StorageError::NotFound)
    ));
    let audit = f.store.collection_audit(&f.owner, request).await.unwrap();
    assert_eq!(
        audit.iter().map(|a| a.event.as_str()).collect::<Vec<_>>(),
        vec!["draft", "running", "succeeded"]
    );
    assert!(!serde_json::to_string(&audit).unwrap().contains("private"));
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert!(reopened.get_collection(&f.owner, request).await.unwrap() == result);
    let claim = f.claim(&sub).await;
    let same = f
        .store
        .finish_collection(&claim, response(ITEM))
        .await
        .unwrap();
    assert_eq!(same.counts.unchanged, 1);
    let same_entry = f
        .store
        .list_feed_entries(&f.owner, &sub.snapshot.subscription_id, None)
        .await
        .unwrap()
        .items
        .remove(0);
    assert_eq!(same_entry.updated_at_unix_ms, entries[0].updated_at_unix_ms);
    let claim = f.claim(&sub).await;
    let updated = f
        .store
        .finish_collection(&claim, response(&ITEM.replace("Original", "Edited")))
        .await
        .unwrap();
    assert_eq!(updated.counts.updated, 1);
    let edited = f
        .store
        .list_feed_entries(&f.owner, &sub.snapshot.subscription_id, None)
        .await
        .unwrap()
        .items
        .remove(0);
    assert_eq!(edited.first_seen_unix_ms, entries[0].first_seen_unix_ms);
    assert_eq!(edited.entry_key, entries[0].entry_key);
    assert_eq!(edited.title, "Edited");
    let claim = f.claim(&sub).await;
    let failed = f
        .store
        .finish_collection(
            &claim,
            response(&format!(
                "{}<item><title>No identity</title></item>",
                ITEM.replace("stable", "new")
            )),
        )
        .await
        .unwrap();
    assert_eq!(failed.status, CollectionStatus::Failed);
    assert_eq!(failed.reason.as_deref(), Some("parse"));
    assert_eq!(failed.counts, EntryCounts::default());
    assert!(
        f.store
            .list_feed_entries(&f.owner, &sub.snapshot.subscription_id, None)
            .await
            .unwrap()
            .items
            == vec![edited]
    );
    // 同一用户的另一个订阅使用相同 GUID，仍作为独立新条目。
    let second = f.sub().await;
    let claim = f.claim(&second).await;
    assert_eq!(
        f.store
            .finish_collection(&claim, response(ITEM))
            .await
            .unwrap()
            .counts
            .inserted,
        1
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn changed_or_deleted_subscriptions_fence_writeback_and_cannot_resurrect() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let claim = f.claim(&sub).await;
    let mut changed = input();
    changed.source_url = "https://example.com/new".into();
    let updated = f
        .store
        .update_subscription(&f.owner, &sub.snapshot.subscription_id, 1, &changed)
        .await
        .unwrap();
    let failed = f
        .store
        .finish_collection(&claim, response(ITEM))
        .await
        .unwrap();
    assert_eq!(failed.reason.as_deref(), Some("subscription_changed"));
    assert!(
        f.store
            .list_feed_entries(&f.owner, &sub.snapshot.subscription_id, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let claim = f.claim(&updated).await;
    let (finished, deleted) = tokio::join!(
        f.store.finish_collection(&claim, response(ITEM)),
        f.store
            .delete_subscription(&f.owner, &sub.snapshot.subscription_id, 2)
    );
    deleted.unwrap();
    assert!(matches!(
        finished.unwrap().status,
        CollectionStatus::Succeeded | CollectionStatus::Failed
    ));
    assert!(matches!(
        f.store
            .get_subscription(&f.owner, &sub.snapshot.subscription_id)
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store
            .list_feed_entries(&f.owner, &sub.snapshot.subscription_id, None)
            .await,
        Err(StorageError::NotFound)
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM feed_entries WHERE user_id=$1")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    is_conflict(
        f.store
            .create_subscription(&f.owner, &sub.snapshot.subscription_id, &input())
            .await,
    );
    is_conflict(
        f.store
            .preview_collection(
                &f.owner,
                &sub.snapshot.subscription_id,
                &Uuid::new_v4().to_string(),
            )
            .await,
    );
    f.store
        .delete_subscription(&f.owner, &sub.snapshot.subscription_id, 2)
        .await
        .unwrap();
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn expired_running_work_becomes_unknown_without_reclaim_or_refund() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let claim = f.claim(&sub).await;
    is_conflict(
        f.store
            .recover_collection(&f.owner, &claim.request_id)
            .await,
    );
    sqlx::query("UPDATE feed_collections SET claimed_ms=claimed_ms-60001,deadline_ms=deadline_ms-60001 WHERE user_id=$1 AND request_id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&claim.request_id).unwrap()).execute(&f.pool).await.unwrap();
    let unknown = f
        .store
        .recover_collection(&f.owner, &claim.request_id)
        .await
        .unwrap();
    assert_eq!(unknown.status, CollectionStatus::Unknown);
    assert!(unknown.claimed_at_unix_ms.is_some());
    assert!(
        f.store
            .recover_collection(&f.owner, &claim.request_id)
            .await
            .unwrap()
            == unknown
    );
    assert!(
        f.store
            .finish_collection(&claim, response(ITEM))
            .await
            .unwrap()
            == unknown
    );
    is_conflict(
        f.store
            .claim_collection(&f.owner, &claim.request_id, &unknown.digest)
            .await,
    );
    assert!(
        f.store
            .list_feed_entries(&f.owner, &sub.snapshot.subscription_id, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        f.store
            .collection_audit(&f.owner, &claim.request_id)
            .await
            .unwrap()
            .len(),
        3
    );
    let fresh = f.claim(&sub).await;
    assert_eq!(
        f.store
            .finish_collection(
                &fresh,
                CollectionOutcome::Failure(CollectionFailure::Unknown)
            )
            .await
            .unwrap()
            .status,
        CollectionStatus::Unknown
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn daily_claim_quota_counts_failed_attempts_across_subscriptions() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let second = f.sub().await;
    for _ in 0..20 {
        let claim = f.claim(&sub).await;
        f.store
            .finish_collection(
                &claim,
                CollectionOutcome::Failure(CollectionFailure::Transport),
            )
            .await
            .unwrap();
    }
    let draft = f.preview(&second).await;
    is_conflict(
        f.store
            .claim_collection(&f.owner, &draft.plan.request_id, &draft.digest)
            .await,
    );
    let status = f
        .store
        .get_collection(&f.owner, &draft.plan.request_id)
        .await
        .unwrap();
    assert_eq!(status.status, CollectionStatus::Draft);
    assert_eq!(
        f.store
            .collection_audit(&f.owner, &draft.plan.request_id)
            .await
            .unwrap()
            .len(),
        1
    );
    let first = f.store.list_collections(&f.owner, None).await.unwrap();
    assert_eq!(first.items.len(), 20);
    let last = f
        .store
        .list_collections(&f.owner, first.next_cursor.as_deref())
        .await
        .unwrap();
    assert_eq!(last.items.len(), 1);
    assert!(last.next_cursor.is_none());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn active_subscription_quota_is_serialized_and_listing_is_bounded() {
    let f = Fixture::new().await;
    for _ in 0..49 {
        f.sub().await;
    }
    let a = Uuid::new_v4().to_string();
    let b = Uuid::new_v4().to_string();
    let value = input();
    let (a, b) = tokio::join!(
        f.store.create_subscription(&f.owner, &a, &value),
        f.store.create_subscription(&f.owner, &b, &value)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let mut disabled = input();
    disabled.enabled = false;
    let inactive = f
        .store
        .create_subscription(&f.owner, &Uuid::new_v4().to_string(), &disabled)
        .await
        .unwrap();
    is_conflict(
        f.store
            .update_subscription(&f.owner, &inactive.snapshot.subscription_id, 1, &input())
            .await,
    );
    let mut all = Vec::new();
    let mut cursor = None;
    loop {
        let page = f
            .store
            .list_subscriptions(&f.owner, cursor.as_deref())
            .await
            .unwrap();
        assert!(page.items.len() <= 20);
        all.extend(
            page.items
                .iter()
                .map(|s| s.snapshot.subscription_id.clone()),
        );
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(all.len(), 51);
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 51);
    f.store
        .delete_subscription(&f.owner, &all[0], 1)
        .await
        .unwrap();
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn saved_plan_tampering_and_expiration_fail_closed() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    for expired in [false, true] {
        let draft = f.preview(&sub).await;
        let mut plan = draft.plan.clone();
        if expired {
            plan.created_at_unix_ms -= 600_000;
            plan.approval_expires_at_unix_ms -= 600_000;
        } else {
            plan.policy.call_models = true;
        }
        let digest = plan.consent_digest().unwrap();
        sqlx::query("UPDATE feed_collections SET plan=$3,digest=$4,created_ms=$5 WHERE user_id=$1 AND request_id=$2")
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&plan.request_id).unwrap()).bind(serde_json::to_value(&plan).unwrap()).bind(&digest).bind(i64::try_from(plan.created_at_unix_ms).unwrap()).execute(&f.pool).await.unwrap();
        is_conflict(
            f.store
                .claim_collection(&f.owner, &draft.plan.request_id, &digest)
                .await,
        );
        assert_eq!(
            f.store
                .collection_audit(&f.owner, &draft.plan.request_id)
                .await
                .unwrap()
                .len(),
            1
        );
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn writeback_rolls_back_entries_and_status_when_audit_cannot_commit() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let claim = f.claim(&sub).await;
    let owner = Uuid::parse_str(f.owner.as_str()).unwrap();
    let request = Uuid::parse_str(&claim.request_id).unwrap();
    // 注入该请求专属的审计唯一键故障，确保前面的条目和终态更新一起回滚。
    sqlx::query("INSERT INTO feed_collection_audit(user_id,request_id,event,at_ms) VALUES($1,$2,'succeeded',1)")
        .bind(owner).bind(request).execute(&f.pool).await.unwrap();
    assert!(
        f.store
            .finish_collection(&claim, response(ITEM))
            .await
            .is_err()
    );
    assert_eq!(
        f.store
            .get_collection(&f.owner, &claim.request_id)
            .await
            .unwrap()
            .status,
        CollectionStatus::Running
    );
    assert!(
        f.store
            .list_feed_entries(&f.owner, &sub.snapshot.subscription_id, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    sqlx::query("DELETE FROM feed_collection_audit WHERE user_id=$1 AND request_id=$2 AND event='succeeded'")
        .bind(owner).bind(request).execute(&f.pool).await.unwrap();
    assert_eq!(
        f.store
            .finish_collection(&claim, response(ITEM))
            .await
            .unwrap()
            .counts
            .inserted,
        1
    );
    let other_sub = f
        .store
        .create_subscription(&f.other, &sub.snapshot.subscription_id, &input())
        .await
        .unwrap();
    let preview = f
        .store
        .preview_collection(
            &f.other,
            &other_sub.snapshot.subscription_id,
            &claim.request_id,
        )
        .await
        .unwrap();
    let other_claim = f
        .store
        .claim_collection(&f.other, &claim.request_id, &preview.digest)
        .await
        .unwrap();
    let result = f
        .store
        .finish_collection(
            &other_claim,
            response(&ITEM.replace("Original", "Other owner")),
        )
        .await
        .unwrap();
    assert_eq!(result.counts.inserted, 1);
    assert_eq!(
        f.store
            .list_feed_entries(&f.owner, &sub.snapshot.subscription_id, None)
            .await
            .unwrap()
            .items[0]
            .title,
        "Original"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn preview_quota_cannot_be_reset_by_cancellation_or_idempotent_retry() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let first = f.preview(&sub).await;
    for _ in 1..100 {
        let draft = f.preview(&sub).await;
        f.store
            .cancel_collection(&f.owner, &draft.plan.request_id)
            .await
            .unwrap();
    }
    is_conflict(
        f.store
            .preview_collection(
                &f.owner,
                &sub.snapshot.subscription_id,
                &Uuid::new_v4().to_string(),
            )
            .await,
    );
    assert!(
        f.store
            .preview_collection(
                &f.owner,
                &sub.snapshot.subscription_id,
                &first.plan.request_id
            )
            .await
            .unwrap()
            == first
    );
    f.cleanup().await;
}

#[path = "feeds/executor.rs"]
mod executor;
