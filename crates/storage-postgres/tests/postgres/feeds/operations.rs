use super::*;
use personal_ai_storage::feed_operations::FeedOperationsStore;

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn feed_operations_is_private_read_only_and_reports_overdue_without_recovery() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let claim = f.claim(&sub).await;
    sqlx::query("UPDATE feed_collections SET claimed_ms=claimed_ms-60001,deadline_ms=deadline_ms-60001,created_ms=created_ms-60001 WHERE user_id=$1")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).execute(&f.pool).await.unwrap();
    sqlx::query("UPDATE feed_collection_audit SET at_ms=at_ms-60001 WHERE user_id=$1")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    let report = f.store.audit_feeds(&f.owner, None).await.unwrap();
    assert!(report.consistent);
    assert_eq!(report.counts["overdue_running"], 1);
    assert_eq!(report.warnings, ["overdue_running"]);
    assert_eq!(report.remaining_previews, 99);
    assert_eq!(report.items[0].status, "running");
    let json = serde_json::to_string(&report).unwrap();
    for secret in [
        "private=yes",
        "source_url",
        "claim_id",
        "accepted_digest",
        "plan",
        "新闻",
    ] {
        assert!(!json.contains(secret));
    }
    let status: String = sqlx::query_scalar(
        "SELECT status FROM feed_collections WHERE user_id=$1 AND request_id=$2",
    )
    .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
    .bind(Uuid::parse_str(&claim.request_id).unwrap())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(status, "running");
    let other = f.store.audit_feeds(&f.other, None).await.unwrap();
    assert_eq!(other.counts["collections"], 0);
    assert!(other.items.is_empty());
    assert!(matches!(
        f.store
            .audit_feeds(&UserId::new(Uuid::new_v4().to_string()), None)
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(f.store.audit_feeds(&f.owner, Some("bad")).await.is_err());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn feed_operations_detects_missing_and_mismatched_audit_and_deleted_entries() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let claim = f.claim(&sub).await;
    f.store
        .finish_collection(&claim, response(ITEM))
        .await
        .unwrap();
    assert!(
        f.store
            .audit_feeds(&f.owner, None)
            .await
            .unwrap()
            .consistent
    );
    let owner = Uuid::parse_str(f.owner.as_str()).unwrap();
    sqlx::query(
        "UPDATE feed_collection_audit SET inserted=0 WHERE user_id=$1 AND event='succeeded'",
    )
    .bind(owner)
    .execute(&f.pool)
    .await
    .unwrap();
    let report = f.store.audit_feeds(&f.owner, None).await.unwrap();
    assert!(!report.consistent);
    assert_eq!(report.items[0].issues, ["audit_mismatch"]);
    sqlx::query("DELETE FROM feed_collection_audit WHERE user_id=$1 AND event='draft'")
        .bind(owner)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE feed_subscriptions SET enabled=false,deleted=true WHERE user_id=$1")
        .bind(owner)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE feed_entries SET updated_ms=first_seen_ms-1 WHERE user_id=$1")
        .bind(owner)
        .execute(&f.pool)
        .await
        .unwrap();
    let report = f.store.audit_feeds(&f.owner, None).await.unwrap();
    assert_eq!(report.counts["inconsistent_collections"], 1);
    assert_eq!(report.counts["entries_on_deleted_subscriptions"], 1);
    assert_eq!(report.counts["invalid_entry_times"], 1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn feed_operations_pages_metadata_but_keeps_whole_owner_totals() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    // Seed historical drafts directly to exercise pages without bypassing live preview quotas.
    let template = f.preview(&sub).await;
    sqlx::query("INSERT INTO feed_collections(user_id,request_id,subscription_id,plan,digest,created_ms) SELECT user_id,gen_random_uuid(),subscription_id,plan,digest,created_ms-86400001 FROM feed_collections CROSS JOIN generate_series(1,100) WHERE user_id=$1 AND request_id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&template.plan.request_id).unwrap()).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO feed_collection_audit(user_id,request_id,event,at_ms) SELECT user_id,request_id,'draft',created_ms FROM feed_collections WHERE user_id=$1 ON CONFLICT DO NOTHING")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).execute(&f.pool).await.unwrap();
    let first = f.store.audit_feeds(&f.owner, None).await.unwrap();
    assert!(first.consistent);
    assert_eq!(first.items.len(), 100);
    assert_eq!(first.counts["expired_drafts"], 100);
    let last = f
        .store
        .audit_feeds(&f.owner, first.next_cursor.as_deref())
        .await
        .unwrap();
    assert_eq!(last.items.len(), 1);
    assert_eq!(last.counts, first.counts);
    assert!(last.next_cursor.is_none());
    assert!(
        first
            .items
            .iter()
            .all(|i| i.request_id < last.items[0].request_id)
    );
    f.cleanup().await;
}
