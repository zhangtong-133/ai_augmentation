use super::*;
use personal_ai_storage::{brief_schedules::BriefScheduleStore, briefs::BriefStore};
async fn due(f: &Fixture, owner: &UserId) {
    sqlx::query("UPDATE feed_brief_schedules SET next_run_ms=0 WHERE user_id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn automatic_briefs_are_opt_in_atomic_private_daily_and_skip_locked_owners() {
    let f = Fixture::new().await;
    assert!(!f.store.get_brief_schedule(&f.owner).await.unwrap().enabled);
    let enabled = f
        .store
        .save_brief_schedule(&f.owner, 0, true, 0)
        .await
        .unwrap();
    assert_eq!(enabled.revision, 1);
    assert_eq!(
        f.store
            .save_brief_schedule(&f.owner, 0, true, 0)
            .await
            .unwrap(),
        enabled
    );
    assert!(!f.store.generate_due_brief().await.unwrap());
    assert!(matches!(
        f.store.save_brief_schedule(&f.owner, 0, false, 0).await,
        Err(StorageError::Conflict(_))
    ));
    assert!(
        f.store
            .save_brief_schedule(&f.owner, 1, true, 1440)
            .await
            .is_err()
    );
    assert!(!f.store.get_brief_schedule(&f.other).await.unwrap().enabled);
    // 事务提交失败不能留下半份日报或已消耗的调度日。
    due(&f, &f.owner).await;
    let name = format!("brief_block_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("ALTER TABLE feed_brief_schedules ADD CONSTRAINT {name} CHECK (user_id<>'{}'::uuid OR last_attempt_ms IS NULL)", f.owner))
        .execute(&f.pool).await.unwrap();
    assert!(f.store.generate_due_brief().await.is_err());
    assert_eq!(
        f.store
            .list_briefs(&f.owner, None)
            .await
            .unwrap()
            .items
            .len(),
        0
    );
    assert_eq!(
        f.store
            .get_brief_schedule(&f.owner)
            .await
            .unwrap()
            .next_run_unix_ms,
        Some(0)
    );
    sqlx::query(&format!(
        "ALTER TABLE feed_brief_schedules DROP CONSTRAINT {name}"
    ))
    .execute(&f.pool)
    .await
    .unwrap();
    f.store
        .save_brief_preferences(&f.owner, 0, &["Rust".into()])
        .await
        .unwrap();
    let (a, b) = tokio::join!(f.store.generate_due_brief(), f.store.generate_due_brief());
    assert_eq!(u8::from(a.unwrap()) + u8::from(b.unwrap()), 1);
    let saved = f.store.get_brief_schedule(&f.owner).await.unwrap();
    assert_eq!(saved.last_outcome.as_deref(), Some("generated"));
    let request = saved.last_request_id.unwrap();
    let brief = f.store.get_brief(&f.owner, &request).await.unwrap();
    assert_eq!(brief.preference_revision, 1);
    assert_eq!(brief.plan.unwrap().keywords, vec!["rust"]);
    assert!(matches!(
        f.store.get_brief(&f.other, &request).await,
        Err(StorageError::NotFound)
    ));
    f.store.delete_brief(&f.owner, &request).await.unwrap();
    // 停用/重新启用或修改时刻不能在当天生成第二份或复活删除内容。
    let disabled = f
        .store
        .save_brief_schedule(&f.owner, 1, false, 0)
        .await
        .unwrap();
    assert_eq!(disabled.next_run_unix_ms, None);
    f.store
        .save_brief_schedule(&f.owner, disabled.revision, true, 0)
        .await
        .unwrap();
    assert!(
        f.store
            .save_brief_schedule(&f.owner, 0, true, 0)
            .await
            .is_err()
    );
    due(&f, &f.owner).await;
    assert!(f.store.generate_due_brief().await.unwrap());
    assert_eq!(
        f.store
            .list_briefs(&f.owner, None)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert!(
        f.store
            .get_brief(&f.owner, &request)
            .await
            .unwrap()
            .plan
            .is_none()
    );
    // 拿不到用户锁时跳过，另一个用户仍能执行。
    f.store
        .save_brief_schedule(&f.other, 0, true, 0)
        .await
        .unwrap();
    due(&f, &f.owner).await;
    due(&f, &f.other).await;
    let mut lock = f.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .execute(&mut *lock)
        .await
        .unwrap();
    assert!(f.store.generate_due_brief().await.unwrap());
    assert_eq!(
        f.store
            .get_brief_schedule(&f.other)
            .await
            .unwrap()
            .last_outcome
            .as_deref(),
        Some("generated")
    );
    assert!(!f.store.generate_due_brief().await.unwrap());
    lock.rollback().await.unwrap();
    f.store
        .save_brief_schedule(&f.owner, 3, false, 0)
        .await
        .unwrap();
    assert!(!f.store.generate_due_brief().await.unwrap());
    f.cleanup().await;

    let f = Fixture::new().await;
    // 配额满时只记录跳过，不影响其他用户，不在当天重试。
    for _ in 0..10 {
        f.store
            .create_brief(&f.owner, &Uuid::new_v4().to_string(), 0)
            .await
            .unwrap();
    }
    f.store
        .save_brief_schedule(&f.owner, 0, true, 0)
        .await
        .unwrap();
    due(&f, &f.owner).await;
    assert!(f.store.generate_due_brief().await.unwrap());
    let skipped = f.store.get_brief_schedule(&f.owner).await.unwrap();
    assert_eq!(skipped.last_outcome.as_deref(), Some("skipped"));
    assert_eq!(skipped.last_request_id, None);
    assert!(!f.store.generate_due_brief().await.unwrap());
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
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM feed_brief_schedules WHERE user_id=$1")
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        0
    );
}
