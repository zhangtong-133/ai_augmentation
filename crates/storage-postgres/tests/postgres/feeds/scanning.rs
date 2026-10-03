use super::*;
use personal_ai_storage::feed_schedules::{
    FeedScheduleExecutionStore, FeedScheduleScanStore, FeedScheduleStore,
};
use std::collections::HashSet;

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn due_scan_pages_across_owners_and_excludes_consumed_cancelled_and_future_slots() {
    let a = Fixture::new().await;
    let b = Fixture::new().await;
    let mut expected = HashSet::new();
    for f in [&a, &b] {
        for _ in 0..11 {
            let sub = f.sub().await;
            let saved = super::scheduled_execution::due(f, &sub).await;
            expected.insert((f.owner.as_str().to_owned(), saved.plan.input.schedule_id));
        }
    }
    let sub = a.sub().await;
    let cancelled = super::scheduled_execution::due(&a, &sub).await;
    a.store
        .cancel_feed_schedule(&a.owner, &cancelled.plan.input.schedule_id)
        .await
        .unwrap();
    let future = super::schedules::draft(&a, &sub).await;
    a.store
        .approve_feed_schedule(&a.owner, &future.plan.input.schedule_id, &future.digest)
        .await
        .unwrap();
    let mut cursor = None;
    let mut seen = HashSet::new();
    loop {
        let page = a
            .store
            .scan_due_feed_schedules(cursor.as_ref())
            .await
            .unwrap();
        assert!(page.items.len() <= 20);
        for item in page.items {
            if item.owner == a.owner || item.owner == b.owner {
                assert!(expected.contains(&(item.owner.as_str().to_owned(), item.id.clone())));
                assert!(seen.insert((item.owner.as_str().to_owned(), item.id)));
            }
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen, expected);
    let target = expected
        .iter()
        .find(|(owner, _)| owner == a.owner.as_str())
        .unwrap()
        .1
        .clone();
    let claim = a
        .store
        .claim_scheduled_collection(&a.owner, &target)
        .await
        .unwrap();
    let page = a.store.scan_due_feed_schedules(None).await.unwrap();
    assert!(
        !page
            .items
            .iter()
            .any(|item| item.owner == a.owner && item.id == target)
    );
    a.store
        .finish_collection(
            &claim,
            CollectionOutcome::Failure(CollectionFailure::Unknown),
        )
        .await
        .unwrap();
    a.cleanup().await;
    b.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn recovery_scan_finds_only_expired_automatic_requests_even_after_authorization_cancelled() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let saved = super::scheduled_execution::due(&f, &sub).await;
    let claim = f
        .store
        .claim_scheduled_collection(&f.owner, &saved.plan.input.schedule_id)
        .await
        .unwrap();
    let page = f
        .store
        .scan_expired_scheduled_collections(None)
        .await
        .unwrap();
    assert!(!page.items.iter().any(|item| item.owner == f.owner));
    f.store
        .cancel_feed_schedule(&f.owner, &saved.plan.input.schedule_id)
        .await
        .unwrap();
    sqlx::query("UPDATE feed_collections SET claimed_ms=claimed_ms-120000,deadline_ms=deadline_ms-120000 WHERE user_id=$1").bind(Uuid::parse_str(f.owner.as_str()).unwrap()).execute(&f.pool).await.unwrap();
    let mut cursor = None;
    let mut found = false;
    loop {
        let page = f
            .store
            .scan_expired_scheduled_collections(cursor.as_ref())
            .await
            .unwrap();
        found |= page
            .items
            .iter()
            .any(|item| item.owner == f.owner && item.id == claim.request_id);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert!(found);
    f.store
        .recover_collection(&f.owner, &claim.request_id)
        .await
        .unwrap();
    let manual = f.claim(&sub).await;
    sqlx::query("UPDATE feed_collections SET claimed_ms=claimed_ms-120000,deadline_ms=deadline_ms-120000 WHERE user_id=$1 AND request_id=$2").bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&manual.request_id).unwrap()).execute(&f.pool).await.unwrap();
    assert!(
        !f.store
            .scan_expired_scheduled_collections(None)
            .await
            .unwrap()
            .items
            .iter()
            .any(|item| item.owner == f.owner)
    );
    f.cleanup().await;
}
