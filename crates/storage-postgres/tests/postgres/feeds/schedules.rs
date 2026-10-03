use super::*;
use personal_ai_feeds::schedule::{ScheduleInput, SchedulePlan};
use personal_ai_storage::feed_schedules::{FeedSchedule, FeedScheduleStatus, FeedScheduleStore};

pub(super) async fn draft(f: &Fixture, sub: &Subscription) -> FeedSchedule {
    let time: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    f.store
        .preview_feed_schedule(
            &f.owner,
            &sub.snapshot.subscription_id,
            &ScheduleInput {
                schedule_id: Uuid::new_v4().to_string(),
                starts_at_unix_ms: u64::try_from(time + 600_000).unwrap(),
                ends_at_unix_ms: u64::try_from(time + 86_400_000).unwrap(),
                interval_hours: 1,
            },
        )
        .await
        .unwrap()
}
async fn age(f: &Fixture, saved: &FeedSchedule) {
    let mut plan: SchedulePlan = saved.plan.clone();
    let delta = 8 * 86_400_000;
    plan.created_at_unix_ms -= delta;
    plan.approval_expires_at_unix_ms -= delta;
    plan.input.starts_at_unix_ms -= delta;
    plan.input.ends_at_unix_ms -= delta;
    sqlx::query("UPDATE feed_schedules SET plan=$3,digest=$4,created_ms=$5,approval_expires_ms=$6,ends_ms=$7,approved_ms=approved_ms-$8 WHERE user_id=$1 AND id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&plan.input.schedule_id).unwrap())
        .bind(serde_json::to_value(&plan).unwrap()).bind(plan.consent_digest().unwrap())
        .bind(i64::try_from(plan.created_at_unix_ms).unwrap()).bind(i64::try_from(plan.approval_expires_at_unix_ms).unwrap()).bind(i64::try_from(plan.input.ends_at_unix_ms).unwrap()).bind(i64::try_from(delta).unwrap()).execute(&f.pool).await.unwrap();
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn schedule_consent_persists_is_private_and_cancellation_never_resurrects() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let saved = draft(&f, &sub).await;
    let id = &saved.plan.input.schedule_id;
    let replay = f
        .store
        .preview_feed_schedule(&f.owner, &sub.snapshot.subscription_id, &saved.plan.input)
        .await
        .unwrap();
    assert!(replay.plan == saved.plan);
    assert_eq!(replay.digest, saved.digest);
    let mut changed = saved.plan.input.clone();
    changed.interval_hours = 6;
    assert!(matches!(
        f.store
            .preview_feed_schedule(&f.owner, &sub.snapshot.subscription_id, &changed)
            .await,
        Err(StorageError::Conflict(_))
    ));
    assert!(matches!(
        f.store.approve_feed_schedule(&f.owner, id, "wrong").await,
        Err(StorageError::Conflict(_))
    ));
    assert!(matches!(
        f.store.get_feed_schedule(&f.other, id).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store
            .approve_feed_schedule(&f.other, id, &saved.digest)
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store.cancel_feed_schedule(&f.other, id).await,
        Err(StorageError::NotFound)
    ));
    assert!(
        f.store
            .list_feed_schedules(&f.other, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let active = f
        .store
        .approve_feed_schedule(&f.owner, id, &saved.digest)
        .await
        .unwrap();
    assert_eq!(active.status, FeedScheduleStatus::Active);
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .approve_feed_schedule(&f.owner, id, &saved.digest)
            .await
            .unwrap()
            .approved_at_unix_ms,
        active.approved_at_unix_ms
    );
    let other = draft(&f, &sub).await;
    assert!(matches!(
        f.store
            .approve_feed_schedule(&f.owner, &other.plan.input.schedule_id, &other.digest)
            .await,
        Err(StorageError::Conflict(_))
    ));
    for _ in 0..2 {
        assert_eq!(
            f.store
                .cancel_feed_schedule(&f.owner, id)
                .await
                .unwrap()
                .status,
            FeedScheduleStatus::Cancelled
        );
    }
    assert!(matches!(
        f.store
            .approve_feed_schedule(&f.owner, id, &saved.digest)
            .await,
        Err(StorageError::Conflict(_))
    ));
    let events = f.store.feed_schedule_audit(&f.owner, id).await.unwrap();
    assert_eq!(events.len(), 3);
    let text = serde_json::to_string(&events).unwrap();
    assert!(!text.contains("private"));
    assert!(!text.contains("https"));
    assert!(matches!(
        f.store.feed_schedule_audit(&f.other, id).await,
        Err(StorageError::NotFound)
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn subscription_changes_and_approval_cancel_races_leave_no_active_authorization() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let saved = draft(&f, &sub).await;
    let id = &saved.plan.input.schedule_id;
    let (_, cancel) = tokio::join!(
        f.store.approve_feed_schedule(&f.owner, id, &saved.digest),
        f.store.cancel_feed_schedule(&f.owner, id)
    );
    assert_eq!(cancel.unwrap().status, FeedScheduleStatus::Cancelled);
    assert_eq!(
        f.store
            .get_feed_schedule(&f.owner, id)
            .await
            .unwrap()
            .status,
        FeedScheduleStatus::Cancelled
    );
    let saved = draft(&f, &sub).await;
    let mut edited = input();
    edited.name = "修改名称".into();
    let (_, updated) = tokio::join!(
        f.store
            .approve_feed_schedule(&f.owner, &saved.plan.input.schedule_id, &saved.digest),
        f.store.update_subscription(
            &f.owner,
            &sub.snapshot.subscription_id,
            sub.snapshot.revision,
            &edited
        )
    );
    let updated = updated.unwrap();
    assert_eq!(
        f.store
            .get_feed_schedule(&f.owner, &saved.plan.input.schedule_id)
            .await
            .unwrap()
            .status,
        FeedScheduleStatus::Cancelled
    );
    let fresh = draft(&f, &updated).await;
    f.store
        .approve_feed_schedule(&f.owner, &fresh.plan.input.schedule_id, &fresh.digest)
        .await
        .unwrap();
    f.store
        .delete_subscription(
            &f.owner,
            &updated.snapshot.subscription_id,
            updated.snapshot.revision,
        )
        .await
        .unwrap();
    assert_eq!(
        f.store
            .get_feed_schedule(&f.owner, &fresh.plan.input.schedule_id)
            .await
            .unwrap()
            .status,
        FeedScheduleStatus::Cancelled
    );
    assert!(matches!(
        f.store
            .approve_feed_schedule(&f.owner, &fresh.plan.input.schedule_id, &fresh.digest)
            .await,
        Err(StorageError::Conflict(_))
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn schedule_expiry_is_durable_and_releases_the_active_subscription_slot() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let saved = draft(&f, &sub).await;
    f.store
        .approve_feed_schedule(&f.owner, &saved.plan.input.schedule_id, &saved.digest)
        .await
        .unwrap();
    age(&f, &saved).await;
    let expired = f
        .store
        .get_feed_schedule(&f.owner, &saved.plan.input.schedule_id)
        .await
        .unwrap();
    assert_eq!(expired.status, FeedScheduleStatus::Expired);
    assert!(matches!(
        f.store
            .approve_feed_schedule(&f.owner, &expired.plan.input.schedule_id, &expired.digest)
            .await,
        Err(StorageError::Conflict(_))
    ));
    let fresh = draft(&f, &sub).await;
    f.store
        .approve_feed_schedule(&f.owner, &fresh.plan.input.schedule_id, &fresh.digest)
        .await
        .unwrap();
    let unapproved = draft(&f, &sub).await;
    age(&f, &unapproved).await;
    assert_eq!(
        f.store
            .get_feed_schedule(&f.owner, &unapproved.plan.input.schedule_id)
            .await
            .unwrap()
            .status,
        FeedScheduleStatus::Expired
    );
    let events = f
        .store
        .feed_schedule_audit(&f.owner, &saved.plan.input.schedule_id)
        .await
        .unwrap();
    assert_eq!(events.iter().filter(|e| e.event == "expired").count(), 1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn schedule_preview_quota_serializes_and_history_paginates_without_duplicates() {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let first = draft(&f, &sub).await;
    for _ in 0..18 {
        draft(&f, &sub).await;
    }
    let mut a = first.plan.input.clone();
    a.schedule_id = Uuid::new_v4().to_string();
    let mut b = a.clone();
    b.schedule_id = Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        f.store
            .preview_feed_schedule(&f.owner, &sub.snapshot.subscription_id, &a),
        f.store
            .preview_feed_schedule(&f.owner, &sub.snapshot.subscription_id, &b)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let failure = if a.is_err() {
        a.err().unwrap()
    } else {
        b.err().unwrap()
    };
    assert!(matches!(failure, StorageError::Conflict(_)));
    age(&f, &first).await;
    draft(&f, &sub).await;
    let page = f.store.list_feed_schedules(&f.owner, None).await.unwrap();
    assert_eq!(page.items.len(), 20);
    let next = f
        .store
        .list_feed_schedules(&f.owner, page.next_cursor.as_deref())
        .await
        .unwrap();
    assert_eq!(next.items.len(), 1);
    assert!(next.next_cursor.is_none());
    assert!(
        !page
            .items
            .iter()
            .any(|s| s.plan.input.schedule_id == next.items[0].plan.input.schedule_id)
    );
    f.cleanup().await;
}
