use super::*;
use personal_ai_agent_core::schedules::{deliver_due_reminders, schedule_digest};
use personal_ai_storage::schedules::{ScheduleDeliveryStore, ScheduleLease};

// 跨用户扫描测试串行，防止测试进程彼此领取夹具；生产并发在各用例内部验证。
static DELIVERY_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn due(f: &Fixture, owner: &UserId) -> Schedule {
    let input = f.input();
    let saved = f.store.create_schedule(owner, &input).await.unwrap();
    f.store
        .approve_schedule(owner, &input.request_id, &approval(&saved))
        .await
        .unwrap();
    // 同时回拨完整冻结输入和授权时间，模拟已到期任务而不等待一分钟。
    let input = NewSchedule {
        run_at_unix_ms: f.now - 1000,
        ..input
    };
    let digest = schedule_digest(owner, &input);
    let consent = ScheduleApproval {
        digest: digest.clone(),
        accepted_run_at_unix_ms: input.run_at_unix_ms,
        ..approval(&saved)
    };
    sqlx::query("UPDATE schedules SET run_at_ms=$3,digest=$4,approval=$5,created_ms=$3-3600000,approval_expires_ms=$3-1800000,approved_ms=$3-3500000 WHERE user_id=$1 AND request_id=$2")
        .bind(Uuid::parse_str(owner.as_str()).unwrap()).bind(Uuid::parse_str(&input.request_id).unwrap()).bind(input.run_at_unix_ms).bind(digest).bind(serde_json::to_value(consent).unwrap()).execute(&f.pool).await.unwrap();
    f.store
        .get_schedule(owner, &input.request_id)
        .await
        .unwrap()
}
async fn count(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM schedule_reminders WHERE user_id=$1")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .fetch_one(&f.pool)
        .await
        .unwrap()
}
async fn claim(f: &Fixture) -> ScheduleLease {
    f.store.claim_due_schedule().await.unwrap().unwrap()
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn scheduler_claims_once_recovers_expired_lease_and_delivers_idempotently() {
    let _guard = DELIVERY_TEST.lock().await;
    let f = Fixture::new().await;
    let future = f.store.create_schedule(&f.owner, &f.input()).await.unwrap();
    f.store
        .approve_schedule(&f.owner, &future.request_id, &approval(&future))
        .await
        .unwrap();
    assert!(f.store.claim_due_schedule().await.unwrap().is_none());
    let saved = due(&f, &f.owner).await;
    let (a, b) = tokio::join!(f.store.claim_due_schedule(), f.store.claim_due_schedule());
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(usize::from(a.is_some()) + usize::from(b.is_some()), 1);
    let old = a.or(b).unwrap();
    sqlx::query("UPDATE schedules SET lease_until_ms=1 WHERE user_id=$1 AND status='running'")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(f.store.deliver_schedule(&old).await.is_err());
    let new = claim(&f).await;
    assert_ne!(old.claim_id, new.claim_id);
    assert!(f.store.deliver_schedule(&old).await.is_err());
    let (a, b) = tokio::join!(
        f.store.deliver_schedule(&new),
        f.store.deliver_schedule(&new)
    );
    let reminder = a.unwrap();
    assert_eq!(reminder, b.unwrap());
    assert_eq!(reminder.body, saved.body);
    assert_eq!(count(&f).await, 1);
    let reopened = PostgresStore::connect_existing(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert_eq!(reminder, reopened.deliver_schedule(&new).await.unwrap());
    let cancelled = f
        .store
        .cancel_schedule(&f.owner, &saved.request_id)
        .await
        .unwrap();
    assert_eq!(cancelled.status, "delivered");
    assert!(cancelled.cancelled_at_unix_ms.is_none());
    assert_eq!(
        f.store
            .approve_schedule(&f.owner, &saved.request_id, &approval(&saved))
            .await
            .unwrap()
            .status,
        "delivered"
    );
    assert_eq!(
        f.store
            .list_schedule_reminders(&f.other, None)
            .await
            .unwrap()
            .items,
        [] as [personal_ai_storage::schedules::ScheduleReminder; 0]
    );
    let forged = ScheduleLease {
        owner: f.other.clone(),
        ..new
    };
    assert!(matches!(
        f.store.deliver_schedule(&forged).await,
        Err(StorageError::NotFound)
    ));
    f.cleanup().await;
    assert_eq!(count(&f).await, 0);
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn scheduler_cancel_and_delivery_serialize_without_late_notifications() {
    let _guard = DELIVERY_TEST.lock().await;
    let f = Fixture::new().await;
    for _ in 0..10 {
        let saved = due(&f, &f.owner).await;
        let lease = claim(&f).await;
        let (delivered, cancelled) = tokio::join!(
            f.store.deliver_schedule(&lease),
            f.store.cancel_schedule(&f.owner, &saved.request_id)
        );
        let cancelled = cancelled.unwrap();
        if cancelled.status == "cancelled" {
            assert!(delivered.is_err());
            assert!(f.store.deliver_schedule(&lease).await.is_err());
        } else {
            assert_eq!(cancelled.status, "delivered");
            assert!(delivered.is_ok());
        }
    }
    let saved = due(&f, &f.owner).await;
    let lease = claim(&f).await;
    f.store
        .cancel_schedule(&f.owner, &saved.request_id)
        .await
        .unwrap();
    let before = count(&f).await;
    assert!(f.store.deliver_schedule(&lease).await.is_err());
    assert_eq!(count(&f).await, before);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn scheduler_rechecks_saved_consent_before_claim_and_delivery() {
    let _guard = DELIVERY_TEST.lock().await;
    let f = Fixture::new().await;
    for after_claim in [false, true] {
        let saved = due(&f, &f.owner).await;
        let lease = if after_claim {
            Some(claim(&f).await)
        } else {
            None
        };
        sqlx::query("UPDATE schedules SET approval=jsonb_set(approval,'{accepted_max_runs}','2') WHERE user_id=$1 AND request_id=$2")
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&saved.request_id).unwrap()).execute(&f.pool).await.unwrap();
        if let Some(lease) = lease {
            assert!(f.store.deliver_schedule(&lease).await.is_err());
        } else {
            assert!(f.store.claim_due_schedule().await.unwrap().is_none());
        }
        assert_eq!(
            f.store
                .get_schedule(&f.owner, &saved.request_id)
                .await
                .unwrap()
                .status,
            "failed"
        );
        assert_eq!(count(&f).await, 0);
    }
    let saved = due(&f, &f.owner).await;
    let lease = claim(&f).await;
    sqlx::query("UPDATE schedules SET body='tampered' WHERE user_id=$1 AND request_id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .bind(Uuid::parse_str(&saved.request_id).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(f.store.deliver_schedule(&lease).await.is_err());
    assert_eq!(count(&f).await, 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn scheduler_skips_locked_owners_and_batch_pages_only_owner_reminders() {
    let _guard = DELIVERY_TEST.lock().await;
    let f = Fixture::new().await;
    due(&f, &f.owner).await;
    let other = due(&f, &f.other).await;
    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let lease = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        f.store.claim_due_schedule(),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert_eq!(lease.owner, f.other);
    assert_eq!(lease.request_id, other.request_id);
    f.store.deliver_schedule(&lease).await.unwrap();
    tx.rollback().await.unwrap();
    for _ in 0..20 {
        due(&f, &f.owner).await;
    }
    assert_eq!(deliver_due_reminders(&f.store).await.unwrap(), 21);
    assert_eq!(deliver_due_reminders(&f.store).await.unwrap(), 0);
    let first = f
        .store
        .list_schedule_reminders(&f.owner, None)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 20);
    let second = f
        .store
        .list_schedule_reminders(&f.owner, first.next_cursor.as_deref())
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(second.next_cursor.is_none());
    assert!(first.items.iter().all(|s|s.request_id!=second.items[0].request_id && s.request_id!=other.request_id));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn scheduler_delivery_commit_failure_rolls_back_both_reminder_and_terminal_state() {
    let _guard = DELIVERY_TEST.lock().await;
    let f = Fixture::new().await;
    let saved = due(&f, &f.owner).await;
    let lease = claim(&f).await;
    let trigger = format!("delivery_fail_{}", Uuid::new_v4().simple());
    sqlx::raw_sql(&format!("CREATE FUNCTION {trigger}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture'; END $$; CREATE CONSTRAINT TRIGGER {trigger} AFTER INSERT ON schedule_reminders DEFERRABLE INITIALLY DEFERRED FOR EACH ROW WHEN (NEW.user_id='{}'::uuid) EXECUTE FUNCTION {trigger}()",f.owner.as_str())).execute(&f.pool).await.unwrap();
    assert!(f.store.deliver_schedule(&lease).await.is_err());
    assert_eq!(count(&f).await, 0);
    assert_eq!(
        f.store
            .get_schedule(&f.owner, &saved.request_id)
            .await
            .unwrap()
            .status,
        "running"
    );
    sqlx::raw_sql(&format!(
        "DROP TRIGGER {trigger} ON schedule_reminders; DROP FUNCTION {trigger}()"
    ))
    .execute(&f.pool)
    .await
    .unwrap();
    f.store.deliver_schedule(&lease).await.unwrap();
    assert_eq!(count(&f).await, 1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)] // 完整投递、并发更新与恢复生命周期。
async fn reminder_inbox_is_private_versioned_and_preserves_delivery_after_archiving() {
    let _guard = DELIVERY_TEST.lock().await;
    let f = Fixture::new().await;
    let saved = due(&f, &f.owner).await;
    let lease = claim(&f).await;
    let original = f.store.deliver_schedule(&lease).await.unwrap();
    assert_eq!(original.revision, 0);
    assert!(original.read_at_unix_ms.is_none());
    assert!(original.archived_at_unix_ms.is_none());
    let request = &saved.request_id;
    assert!(matches!(
        f.store
            .update_reminder(&f.other, request, 0, true, false)
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(
        f.store
            .update_reminder(&f.owner, request, -1, true, false)
            .await
            .is_err()
    );
    let (a, b) = tokio::join!(
        f.store.update_reminder(&f.owner, request, 0, true, false),
        f.store.update_reminder(&f.owner, request, 0, false, true)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let changed = a.or(b).unwrap();
    assert_eq!(changed.revision, 1);
    let archived = f
        .store
        .update_reminder(&f.owner, request, 1, true, true)
        .await
        .unwrap();
    assert_eq!(archived.revision, 2);
    assert_eq!(archived.delivered_at_unix_ms, original.delivered_at_unix_ms);
    assert_eq!(archived.body, original.body);
    assert_eq!(
        f.store
            .update_reminder(&f.owner, request, 1, true, true)
            .await
            .unwrap(),
        archived
    );
    assert_eq!(
        f.store
            .list_reminder_inbox(&f.owner, None, Some(false))
            .await
            .unwrap()
            .items
            .len(),
        0
    );
    assert_eq!(
        f.store
            .list_reminder_inbox(&f.owner, None, Some(true))
            .await
            .unwrap()
            .items,
        vec![archived.clone()]
    );
    assert_eq!(
        f.store
            .list_reminder_inbox(&f.other, None, Some(true))
            .await
            .unwrap()
            .items
            .len(),
        0
    );
    // 原租约重放只能读取当前提醒，不重新投递或清除收件箱状态。
    assert_eq!(f.store.deliver_schedule(&lease).await.unwrap(), archived);
    assert_eq!(count(&f).await, 1);
    let restored = f
        .store
        .update_reminder(&f.owner, request, 2, true, false)
        .await
        .unwrap();
    assert_eq!(restored.read_at_unix_ms, archived.read_at_unix_ms);
    assert!(restored.archived_at_unix_ms.is_none());
    assert!(matches!(
        f.store
            .update_reminder(&f.owner, request, 1, true, true)
            .await,
        Err(StorageError::Conflict(_))
    ));
    let unread = f
        .store
        .update_reminder(&f.owner, request, 3, false, false)
        .await
        .unwrap();
    assert!(unread.read_at_unix_ms.is_none());
    assert_eq!(unread.revision, 4);
    let reopened = PostgresStore::connect_existing(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .list_reminder_inbox(&f.owner, None, Some(false))
            .await
            .unwrap()
            .items,
        vec![unread]
    );
    assert_eq!(
        f.store
            .get_schedule(&f.owner, request)
            .await
            .unwrap()
            .status,
        "delivered"
    );
    assert!(f.store.claim_due_schedule().await.unwrap().is_none());
    f.cleanup().await;
}
