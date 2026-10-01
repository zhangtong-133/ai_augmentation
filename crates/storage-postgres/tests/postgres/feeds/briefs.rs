use super::*;
use personal_ai_storage::briefs::{BriefStatus, BriefStore};

async fn collected(f: &Fixture) -> Subscription {
    let sub = f.sub().await;
    let claim = f.claim(&sub).await;
    f.store
        .finish_collection(&claim, response(ITEM))
        .await
        .unwrap();
    sub
}
fn owner(f: &Fixture) -> Uuid {
    Uuid::parse_str(f.owner.as_str()).unwrap()
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn brief_preferences_use_normalization_and_compare_and_swap() {
    let f = Fixture::new().await;
    assert_eq!(
        f.store
            .get_brief_preferences(&f.owner)
            .await
            .unwrap()
            .revision,
        0
    );
    assert_eq!(
        f.store
            .save_brief_preferences(&f.owner, 0, &[])
            .await
            .unwrap()
            .revision,
        1
    );
    let keywords = vec![" Rust ".into(), "AI".into()];
    let (a, b) = tokio::join!(
        f.store.save_brief_preferences(&f.owner, 1, &keywords),
        f.store
            .save_brief_preferences(&f.owner, 1, &["other".into()])
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let saved = f.store.get_brief_preferences(&f.owner).await.unwrap();
    assert_eq!(saved.revision, 2);
    assert!(
        f.store
            .save_brief_preferences(&f.owner, 2, &saved.keywords)
            .await
            .unwrap()
            == saved
    );
    is_conflict(f.store.save_brief_preferences(&f.owner, 1, &[]).await);
    assert!(matches!(
        f.store
            .save_brief_preferences(&f.owner, 2, &["x".repeat(65)])
            .await,
        Err(StorageError::InvalidData(_))
    ));
    assert_eq!(
        f.store
            .get_brief_preferences(&f.other)
            .await
            .unwrap()
            .keywords,
        [] as [std::string::String; 0]
    );
    let normalized = f
        .store
        .save_brief_preferences(&f.owner, 2, &keywords)
        .await
        .unwrap();
    assert_eq!(normalized.keywords, vec!["ai", "rust"]);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn brief_snapshots_are_private_frozen_and_idempotent_under_concurrency() {
    let f = Fixture::new().await;
    let sub = collected(&f).await;
    let request = Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        f.store.create_brief(&f.owner, &request, 0),
        f.store.create_brief(&f.owner, &request, 0)
    );
    let saved = a.unwrap();
    assert!(saved == b.unwrap());
    assert_eq!(
        saved.plan.as_ref().unwrap().items[0].entry.title,
        "Original"
    );
    let claim = f.claim(&sub).await;
    f.store
        .finish_collection(&claim, response(&ITEM.replace("Original", "Edited")))
        .await
        .unwrap();
    f.store
        .save_brief_preferences(&f.owner, 0, &["edited".into()])
        .await
        .unwrap();
    assert!(saved == f.store.create_brief(&f.owner, &request, 0).await.unwrap());
    is_conflict(f.store.create_brief(&f.owner, &request, 1).await);
    assert!(matches!(
        f.store.get_brief(&f.other, &request).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store.delete_brief(&f.other, &request).await,
        Err(StorageError::NotFound)
    ));
    assert!(
        f.store
            .list_briefs(&f.other, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert!(saved == reopened.get_brief(&f.owner, &request).await.unwrap());
    sqlx::query(
        "UPDATE feed_briefs SET plan=jsonb_set(plan,'{version}','\"tampered\"') WHERE user_id=$1",
    )
    .bind(owner(&f))
    .execute(&f.pool)
    .await
    .unwrap();
    is_conflict(f.store.get_brief(&f.owner, &request).await);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn brief_source_deletion_clears_even_deduplicated_sources_and_cannot_resurrect() {
    let f = Fixture::new().await;
    let first = collected(&f).await;
    let second = collected(&f).await;
    let request = Uuid::new_v4().to_string();
    let saved = f.store.create_brief(&f.owner, &request, 0).await.unwrap();
    let plan = saved.plan.unwrap();
    assert_eq!(plan.items.len(), 1);
    let omitted = if plan.items[0].entry.subscription_id == first.snapshot.subscription_id {
        &second
    } else {
        &first
    };
    f.store
        .delete_subscription(&f.owner, &omitted.snapshot.subscription_id, 1)
        .await
        .unwrap();
    let invalid = f.store.get_brief(&f.owner, &request).await.unwrap();
    assert_eq!(invalid.status, BriefStatus::Invalidated);
    assert!(invalid.plan.is_none());
    assert!(invalid == f.store.create_brief(&f.owner, &request, 0).await.unwrap());
    let fresh = Uuid::new_v4().to_string();
    f.store.create_brief(&f.owner, &fresh, 0).await.unwrap();
    sqlx::query("DELETE FROM feed_subscriptions WHERE user_id=$1 AND NOT deleted")
        .bind(owner(&f))
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(
        f.store
            .get_brief(&f.owner, &fresh)
            .await
            .unwrap()
            .plan
            .is_none()
    );
    f.store.delete_brief(&f.owner, &request).await.unwrap();
    f.store.delete_brief(&f.owner, &request).await.unwrap();
    assert_eq!(
        f.store
            .create_brief(&f.owner, &request, 0)
            .await
            .unwrap()
            .status,
        BriefStatus::Deleted
    );
    f.cleanup().await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM feed_briefs WHERE user_id=$1")
        .bind(owner(&f))
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn brief_creation_and_source_deletion_serialize_without_retaining_content() {
    let f = Fixture::new().await;
    let sub = collected(&f).await;
    let request = Uuid::new_v4().to_string();
    let (created, deleted) = tokio::join!(
        f.store.create_brief(&f.owner, &request, 0),
        f.store
            .delete_subscription(&f.owner, &sub.snapshot.subscription_id, 1)
    );
    created.unwrap();
    deleted.unwrap();
    let saved = f.store.get_brief(&f.owner, &request).await.unwrap();
    assert!(saved.plan.is_none_or(|p| p.items.is_empty()));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn brief_daily_quota_counts_tombstones_and_allows_original_retries() {
    let f = Fixture::new().await;
    let first = Uuid::new_v4().to_string();
    f.store.create_brief(&f.owner, &first, 0).await.unwrap();
    for _ in 1..10 {
        let request = Uuid::new_v4().to_string();
        f.store.create_brief(&f.owner, &request, 0).await.unwrap();
        f.store.delete_brief(&f.owner, &request).await.unwrap();
    }
    is_conflict(
        f.store
            .create_brief(&f.owner, &Uuid::new_v4().to_string(), 0)
            .await,
    );
    assert_eq!(
        f.store
            .create_brief(&f.owner, &first, 0)
            .await
            .unwrap()
            .status,
        BriefStatus::Ready
    );
    assert_eq!(
        f.store
            .list_briefs(&f.owner, None)
            .await
            .unwrap()
            .items
            .len(),
        10
    );
    f.cleanup().await;
}

async fn seed_entries(f: &Fixture, sub: &Subscription, count: i32, summary_len: i32) {
    sqlx::query("INSERT INTO feed_entries(user_id,subscription_id,entry_key,title,summary,content_digest,first_seen_ms,updated_ms,last_seen_ms) SELECT $1,$2,'guid:'||md5(i::text)||md5(i::text),'Title '||i,repeat('x',$4),repeat('a',64),t,t,t FROM generate_series(1,$3) i CROSS JOIN (SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint t) stamp")
        .bind(owner(f)).bind(Uuid::parse_str(&sub.snapshot.subscription_id).unwrap()).bind(count).bind(summary_len).execute(&f.pool).await.unwrap();
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn brief_bounds_fail_without_partial_records_and_disabled_entries_are_excluded() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    seed_entries(&f, &sub, 501, 0).await;
    let request = Uuid::new_v4().to_string();
    is_conflict(f.store.create_brief(&f.owner, &request, 0).await);
    assert!(matches!(
        f.store.get_brief(&f.owner, &request).await,
        Err(StorageError::NotFound)
    ));
    sqlx::query("DELETE FROM feed_entries WHERE user_id=$1")
        .bind(owner(&f))
        .execute(&f.pool)
        .await
        .unwrap();
    seed_entries(&f, &sub, 300, 8192).await;
    is_conflict(f.store.create_brief(&f.owner, &request, 0).await);
    assert!(
        f.store
            .list_briefs(&f.owner, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let mut disabled = input();
    disabled.enabled = false;
    f.store
        .update_subscription(&f.owner, &sub.snapshot.subscription_id, 1, &disabled)
        .await
        .unwrap();
    assert!(
        f.store
            .create_brief(&f.owner, &request, 0)
            .await
            .unwrap()
            .plan
            .unwrap()
            .items
            .is_empty()
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn brief_history_is_bounded_owner_scoped_and_total_quota_is_persistent() {
    let f = Fixture::new().await;
    sqlx::query("INSERT INTO feed_briefs(user_id,request_id,preference_revision,day_start_ms,created_ms,status,digest) SELECT $1,gen_random_uuid(),0,0,0,'deleted',repeat('a',64) FROM generate_series(1,1000)")
        .bind(owner(&f)).execute(&f.pool).await.unwrap();
    is_conflict(
        f.store
            .create_brief(&f.owner, &Uuid::new_v4().to_string(), 0)
            .await,
    );
    let mut seen = std::collections::BTreeSet::new();
    let mut cursor = None;
    loop {
        let page = f
            .store
            .list_briefs(&f.owner, cursor.as_deref())
            .await
            .unwrap();
        assert!(page.items.len() <= 20);
        for item in page.items {
            assert!(seen.insert(item.request_id));
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 1000);
    assert!(
        f.store
            .list_briefs(&f.other, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn brief_source_write_failure_rolls_back_plan_and_allows_retry() {
    let f = Fixture::new().await;
    collected(&f).await;
    let request = Uuid::new_v4();
    let constraint = format!("brief_fault_{}", request.simple());
    // 只拒绝当前测试请求，故障发生在计划 INSERT 之后的来源 INSERT。
    sqlx::query(&format!("ALTER TABLE feed_brief_sources ADD CONSTRAINT {constraint} CHECK(request_id <> '{request}') NOT VALID"))
        .execute(&f.pool).await.unwrap();
    assert!(
        f.store
            .create_brief(&f.owner, &request.to_string(), 0)
            .await
            .is_err()
    );
    assert!(matches!(
        f.store.get_brief(&f.owner, &request.to_string()).await,
        Err(StorageError::NotFound)
    ));
    sqlx::query(&format!(
        "ALTER TABLE feed_brief_sources DROP CONSTRAINT {constraint}"
    ))
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        f.store
            .create_brief(&f.owner, &request.to_string(), 0)
            .await
            .unwrap()
            .plan
            .unwrap()
            .items
            .len(),
        1
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn brief_uses_first_seen_utc_day_and_account_deletion_removes_ready_snapshots() {
    let f = Fixture::new().await;
    let sub = collected(&f).await;
    let request = Uuid::new_v4().to_string();
    f.store.create_brief(&f.owner, &request, 0).await.unwrap();
    sqlx::query("UPDATE feed_entries SET first_seen_ms=first_seen_ms-86400000 WHERE user_id=$1")
        .bind(owner(&f))
        .execute(&f.pool)
        .await
        .unwrap();
    let newer = f
        .store
        .create_brief(&f.owner, &Uuid::new_v4().to_string(), 0)
        .await
        .unwrap();
    assert!(newer.plan.unwrap().items.is_empty());
    assert_eq!(
        f.store
            .get_brief(&f.owner, &request)
            .await
            .unwrap()
            .plan
            .unwrap()
            .items[0]
            .entry
            .subscription_id,
        sub.snapshot.subscription_id
    );
    f.store
        .save_brief_preferences(&f.owner, 0, &["private".into()])
        .await
        .unwrap();
    f.cleanup().await;
    for table in [
        "feed_briefs",
        "feed_brief_sources",
        "feed_brief_preferences",
    ] {
        let count: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE user_id=$1"))
                .bind(owner(&f))
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }
}
