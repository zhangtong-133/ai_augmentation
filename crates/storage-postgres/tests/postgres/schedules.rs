use super::*;
use personal_ai_storage::schedules::{NewSchedule, Schedule, ScheduleApproval, ScheduleStore};
use sqlx::Row;

struct Fixture {
    store: PostgresStore,
    pool: sqlx::PgPool,
    owner: UserId,
    other: UserId,
    now: i64,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let store = PostgresStore::connect(&url).await.unwrap();
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let mut ids = Vec::new();
        for _ in 0..2 {
            let id = UserId::new(Uuid::new_v4().to_string());
            store
                .save_user(&User {
                    id: id.clone(),
                    email: format!("{}@schedule.example", id.as_str()),
                    display_name: "提醒测试".into(),
                })
                .await
                .unwrap();
            ids.push(id);
        }
        let now =
            sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
                .fetch_one(&pool)
                .await
                .unwrap();
        Self {
            store,
            pool,
            owner: ids.remove(0),
            other: ids.remove(0),
            now,
        }
    }
    fn input(&self) -> NewSchedule {
        NewSchedule {
            request_id: Uuid::new_v4().to_string(),
            title: "复习".into(),
            body: "用户私有提醒，不调用模型".into(),
            run_at_unix_ms: self.now + 3_600_000,
        }
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
fn approval(s: &Schedule) -> ScheduleApproval {
    ScheduleApproval {
        digest: s.digest.clone(),
        accepted_run_at_unix_ms: s.run_at_unix_ms,
        accepted_max_runs: 1,
        accepted_amount_micro: 0,
        acknowledge_schedule: true,
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn schedules_are_private_immutable_idempotent_and_persist_consent() {
    let f = Fixture::new().await;
    let input = f.input();
    let (a, b) = tokio::join!(
        f.store.create_schedule(&f.owner, &input),
        f.store.create_schedule(&f.owner, &input)
    );
    let draft = a.unwrap();
    assert_eq!(draft, b.unwrap());
    assert_eq!(draft.status, "draft");
    assert!(matches!(
        f.store.get_schedule(&f.other, &input.request_id).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store.cancel_schedule(&f.other, &input.request_id).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store
            .approve_schedule(&f.other, &input.request_id, &approval(&draft))
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(
        f.store
            .list_schedules(&f.other, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let mut changed = input.clone();
    changed.body.push('!');
    assert!(matches!(
        f.store.create_schedule(&f.owner, &changed).await,
        Err(StorageError::Conflict(_))
    ));
    let consent = approval(&draft);
    let (a, b) = tokio::join!(
        f.store
            .approve_schedule(&f.owner, &input.request_id, &consent),
        f.store
            .approve_schedule(&f.owner, &input.request_id, &consent)
    );
    let approved = a.unwrap();
    assert_eq!(approved, b.unwrap());
    assert_eq!(approved.status, "scheduled");
    let reopened = PostgresStore::connect_existing(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert_eq!(
        approved,
        reopened
            .get_schedule(&f.owner, &input.request_id)
            .await
            .unwrap()
    );
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT approval FROM schedules WHERE user_id=$1 AND request_id=$2")
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
            .bind(Uuid::parse_str(&input.request_id).unwrap())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(stored, serde_json::to_value(consent).unwrap());
    let cancelled = f
        .store
        .cancel_schedule(&f.owner, &input.request_id)
        .await
        .unwrap();
    assert_eq!(cancelled.status, "cancelled");
    assert_eq!(
        cancelled,
        f.store
            .cancel_schedule(&f.owner, &input.request_id)
            .await
            .unwrap()
    );
    assert_eq!(
        cancelled,
        f.store.create_schedule(&f.owner, &input).await.unwrap()
    );
    assert!(
        f.store
            .approve_schedule(&f.owner, &input.request_id, &approval(&draft))
            .await
            .is_err()
    );
    f.cleanup().await;
    assert!(matches!(
        f.store.get_schedule(&f.owner, &input.request_id).await,
        Err(StorageError::NotFound)
    ));
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn schedules_require_exact_consent_and_reject_expired_or_tampered_drafts() {
    let f = Fixture::new().await;
    let input = f.input();
    let draft = f.store.create_schedule(&f.owner, &input).await.unwrap();
    let good = approval(&draft);
    for index in 0..5 {
        let mut bad = good.clone();
        match index {
            0 => bad.digest.push('0'),
            1 => bad.accepted_run_at_unix_ms += 1,
            2 => bad.accepted_max_runs = 2,
            3 => bad.accepted_amount_micro = 1,
            _ => bad.acknowledge_schedule = false,
        }
        assert!(
            f.store
                .approve_schedule(&f.owner, &input.request_id, &bad)
                .await
                .is_err()
        );
        assert_eq!(
            f.store
                .get_schedule(&f.owner, &input.request_id)
                .await
                .unwrap()
                .status,
            "draft"
        );
    }
    sqlx::query("UPDATE schedules SET created_ms=1,approval_expires_ms=2 WHERE user_id=$1")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .get_schedule(&f.owner, &input.request_id)
            .await
            .unwrap()
            .status,
        "expired"
    );
    assert!(
        f.store
            .approve_schedule(&f.owner, &input.request_id, &good)
            .await
            .is_err()
    );
    let input = f.input();
    let draft = f.store.create_schedule(&f.owner, &input).await.unwrap();
    sqlx::query("UPDATE schedules SET body='changed' WHERE user_id=$1 AND request_id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .bind(Uuid::parse_str(&input.request_id).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(
        f.store
            .approve_schedule(&f.owner, &input.request_id, &approval(&draft))
            .await
            .is_err()
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn schedules_cancel_wins_authorization_races_without_resurrection() {
    let f = Fixture::new().await;
    for _ in 0..10 {
        let input = f.input();
        let draft = f.store.create_schedule(&f.owner, &input).await.unwrap();
        let consent = approval(&draft);
        let (_, cancelled) = tokio::join!(
            f.store
                .approve_schedule(&f.owner, &input.request_id, &consent),
            f.store.cancel_schedule(&f.owner, &input.request_id)
        );
        assert_eq!(cancelled.unwrap().status, "cancelled");
        assert_eq!(
            f.store
                .get_schedule(&f.owner, &input.request_id)
                .await
                .unwrap()
                .status,
            "cancelled"
        );
        assert!(
            f.store
                .approve_schedule(&f.owner, &input.request_id, &consent)
                .await
                .is_err()
        );
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn schedules_bound_creation_and_page_history_without_resetting_cancelled_quota() {
    let f = Fixture::new().await;
    for bad in [f.now, f.now + 366 * 86_400_000, i64::MAX] {
        let mut input = f.input();
        input.run_at_unix_ms = bad;
        assert!(f.store.create_schedule(&f.owner, &input).await.is_err());
    }
    let mut ids = Vec::new();
    for _ in 0..99 {
        let input = f.input();
        f.store.create_schedule(&f.owner, &input).await.unwrap();
        f.store
            .cancel_schedule(&f.owner, &input.request_id)
            .await
            .unwrap();
        ids.push(input.request_id);
    }
    let left = f.input();
    let right = f.input();
    let (a, b) = tokio::join!(
        f.store.create_schedule(&f.owner, &left),
        f.store.create_schedule(&f.owner, &right)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    ids.push(a.or(b).unwrap().request_id);
    ids.sort();
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = f
            .store
            .list_schedules(&f.owner, after.as_deref())
            .await
            .unwrap();
        assert_eq!(page.items.len(), 20);
        seen.extend(page.items.into_iter().map(|s| s.request_id));
        after = page.next_cursor;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(seen, ids);
    let money: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reply_money_reservations WHERE user_id=$1")
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(money, 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn schedules_failed_commit_returns_no_draft_or_authorization() {
    let f = Fixture::new().await;
    let trigger = format!("schedule_fail_{}", Uuid::new_v4().simple());
    let ddl = format!(
        "CREATE FUNCTION {trigger}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture'; END $$; CREATE CONSTRAINT TRIGGER {trigger} AFTER INSERT OR UPDATE ON schedules DEFERRABLE INITIALLY DEFERRED FOR EACH ROW WHEN (NEW.user_id='{}'::uuid) EXECUTE FUNCTION {trigger}()",
        f.owner.as_str()
    );
    sqlx::raw_sql(&ddl).execute(&f.pool).await.unwrap();
    let input = f.input();
    assert!(f.store.create_schedule(&f.owner, &input).await.is_err());
    assert!(matches!(
        f.store.get_schedule(&f.owner, &input.request_id).await,
        Err(StorageError::NotFound)
    ));
    sqlx::raw_sql(&format!("DROP TRIGGER {trigger} ON schedules"))
        .execute(&f.pool)
        .await
        .unwrap();
    let draft = f.store.create_schedule(&f.owner, &input).await.unwrap();
    sqlx::raw_sql(&format!("CREATE CONSTRAINT TRIGGER {trigger} AFTER UPDATE ON schedules DEFERRABLE INITIALLY DEFERRED FOR EACH ROW WHEN (NEW.user_id='{}'::uuid) EXECUTE FUNCTION {trigger}()",f.owner.as_str())).execute(&f.pool).await.unwrap();
    assert!(
        f.store
            .approve_schedule(&f.owner, &input.request_id, &approval(&draft))
            .await
            .is_err()
    );
    let row = sqlx::query("SELECT status,approval FROM schedules WHERE user_id=$1")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("status"), "draft");
    assert!(
        row.get::<Option<serde_json::Value>, _>("approval")
            .is_none()
    );
    sqlx::raw_sql(&format!(
        "DROP TRIGGER {trigger} ON schedules; DROP FUNCTION {trigger}()"
    ))
    .execute(&f.pool)
    .await
    .unwrap();
    f.cleanup().await;
}

#[path = "schedule_delivery.rs"]
mod delivery;
