use super::*;
use personal_ai_storage::feed_operations::FeedOperationsStore;
use personal_ai_storage::feed_schedules::{
    FeedSchedule, FeedScheduleExecutionStore, FeedScheduleStore,
};

// 先完成真实同意，再将整个已授权时间轴平移，避免测试等待十分钟。
pub(super) async fn due(f: &Fixture, sub: &Subscription) -> FeedSchedule {
    let saved = super::schedules::draft(f, sub).await;
    f.store
        .approve_feed_schedule(&f.owner, &saved.plan.input.schedule_id, &saved.digest)
        .await
        .unwrap();
    let mut plan = saved.plan;
    let delta = 630_000_u64;
    plan.created_at_unix_ms -= delta;
    plan.approval_expires_at_unix_ms -= delta;
    plan.input.starts_at_unix_ms -= delta;
    plan.input.ends_at_unix_ms -= delta;
    sqlx::query("UPDATE feed_schedules SET plan=$3,digest=$4,created_ms=$5,approval_expires_ms=$6,ends_ms=$7,approved_ms=approved_ms-$8 WHERE user_id=$1 AND id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&plan.input.schedule_id).unwrap())
        .bind(serde_json::to_value(&plan).unwrap()).bind(plan.consent_digest().unwrap())
        .bind(i64::try_from(plan.created_at_unix_ms).unwrap()).bind(i64::try_from(plan.approval_expires_at_unix_ms).unwrap()).bind(i64::try_from(plan.input.ends_at_unix_ms).unwrap()).bind(i64::try_from(delta).unwrap()).execute(&f.pool).await.unwrap();
    f.store
        .get_feed_schedule(&f.owner, &plan.input.schedule_id)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn scheduled_claim_is_atomic_private_single_dispatch_and_uses_existing_result_pipeline() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let schedule = due(&f, &sub).await;
    let id = &schedule.plan.input.schedule_id;
    assert!(matches!(
        f.store.claim_scheduled_collection(&f.other, id).await,
        Err(StorageError::NotFound)
    ));
    let (a, b) = tokio::join!(
        f.store.claim_scheduled_collection(&f.owner, id),
        f.store.claim_scheduled_collection(&f.owner, id)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let claim = a.or(b).unwrap();
    let manual = f.preview(&sub).await;
    assert!(matches!(
        f.store
            .claim_collection(&f.owner, &manual.plan.request_id, &manual.digest)
            .await,
        Err(StorageError::Conflict(_))
    ));
    let mut forged = claim.clone();
    forged.claim_id = Uuid::new_v4().to_string();
    assert!(
        f.store
            .dispatch_scheduled_collection(&forged)
            .await
            .is_err()
    );
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert!(
        reopened
            .dispatch_scheduled_collection(&claim)
            .await
            .unwrap()
    );
    assert!(!f.store.dispatch_scheduled_collection(&claim).await.unwrap());
    let result = f
        .store
        .finish_collection(
            &claim,
            response("<item><guid>scheduled</guid><title>scheduled item</title></item>"),
        )
        .await
        .unwrap();
    assert_eq!(result.status, CollectionStatus::Succeeded);
    assert_eq!(result.counts.inserted, 1);
    assert!(
        f.store
            .audit_feeds(&f.owner, None)
            .await
            .unwrap()
            .consistent
    );
    assert!(matches!(
        reopened.claim_scheduled_collection(&f.owner, id).await,
        Err(StorageError::Conflict(_))
    ));
    f.store
        .claim_collection(&f.owner, &manual.plan.request_id, &manual.digest)
        .await
        .unwrap();
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn scheduled_cancel_before_or_after_dispatch_discards_results_without_refunding_attempts() {
    let f = Fixture::new().await;
    for send in [false, true] {
        let sub = f.sub().await;
        let schedule = due(&f, &sub).await;
        let claim = f
            .store
            .claim_scheduled_collection(&f.owner, &schedule.plan.input.schedule_id)
            .await
            .unwrap();
        if send {
            assert!(f.store.dispatch_scheduled_collection(&claim).await.unwrap());
        }
        f.store
            .cancel_feed_schedule(&f.owner, &schedule.plan.input.schedule_id)
            .await
            .unwrap();
        assert!(!f.store.dispatch_scheduled_collection(&claim).await.unwrap());
        let result = f
            .store
            .finish_collection(
                &claim,
                response("<item><guid>cancelled</guid><title>must not persist</title></item>"),
            )
            .await
            .unwrap();
        assert_eq!(result.status, CollectionStatus::Failed);
        assert_eq!(result.reason.as_deref(), Some("subscription_changed"));
        assert!(
            f.store
                .list_feed_entries(&f.owner, &sub.snapshot.subscription_id, None)
                .await
                .unwrap()
                .items
                .is_empty()
        );
        assert!(result.claimed_at_unix_ms.is_some());
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn expired_dispatch_and_recovery_never_reclaim_a_scheduled_slot() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let schedule = due(&f, &sub).await;
    let claim = f
        .store
        .claim_scheduled_collection(&f.owner, &schedule.plan.input.schedule_id)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE feed_schedule_occurrences SET dispatch_expires_ms=scheduled_ms+1 WHERE user_id=$1",
    )
    .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
    .execute(&f.pool)
    .await
    .unwrap();
    assert!(!f.store.dispatch_scheduled_collection(&claim).await.unwrap());
    // 模拟领取后崩溃；原请求只恢复为 unknown，时段记录继续占用。
    sqlx::query("UPDATE feed_collections SET claimed_ms=claimed_ms-120000,deadline_ms=deadline_ms-120000 WHERE user_id=$1").bind(Uuid::parse_str(f.owner.as_str()).unwrap()).execute(&f.pool).await.unwrap();
    assert_eq!(
        f.store
            .recover_collection(&f.owner, &claim.request_id)
            .await
            .unwrap()
            .status,
        CollectionStatus::Unknown
    );
    assert!(matches!(
        f.store
            .claim_scheduled_collection(&f.owner, &schedule.plan.input.schedule_id)
            .await,
        Err(StorageError::Conflict(_))
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn manual_and_scheduled_collections_share_daily_quota_and_running_limit() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let schedule = due(&f, &sub).await;
    let first = f.claim(&sub).await;
    assert!(matches!(
        f.store
            .claim_scheduled_collection(&f.owner, &schedule.plan.input.schedule_id)
            .await,
        Err(StorageError::Conflict(_))
    ));
    f.store
        .finish_collection(
            &first,
            CollectionOutcome::Failure(CollectionFailure::Transport),
        )
        .await
        .unwrap();
    for _ in 0..19 {
        let claim = f.claim(&sub).await;
        f.store
            .finish_collection(
                &claim,
                CollectionOutcome::Failure(CollectionFailure::Transport),
            )
            .await
            .unwrap();
    }
    assert!(matches!(
        f.store
            .claim_scheduled_collection(&f.owner, &schedule.plan.input.schedule_id)
            .await,
        Err(StorageError::Conflict(_))
    ));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM feed_schedule_occurrences WHERE user_id=$1")
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    f.cleanup().await;
}
