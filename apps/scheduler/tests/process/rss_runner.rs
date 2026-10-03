use personal_ai_domain::{User, UserId};
use personal_ai_feeds::{
    schedule::ScheduleInput,
    transport::{FeedTransport, FetchError, FetchFuture},
};
use personal_ai_storage::{
    MetadataStore,
    feed_schedules::{FeedScheduleExecutionStore, FeedScheduleStore},
    feeds::{CollectionStatus, FeedStore, SubscriptionInput},
};
use personal_ai_storage_postgres::PostgresStore;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use uuid::Uuid;
struct Transport(AtomicUsize);
impl FeedTransport for Transport {
    fn fetch(&self, _source: &str) -> FetchFuture<'_, Result<Vec<u8>, FetchError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(b"<rss version=\"2.0\"><channel><title>RSS</title><description>fixture</description><link>https://example.org/</link><item><guid>runner</guid><title>runner</title></item></channel></rss>".to_vec())
        })
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)] // 同一生命周期核对禁用、到期恢复、执行和重启去重。
async fn runner_recovers_unknowns_without_resending_and_disabled_process_never_collects() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = Arc::new(PostgresStore::connect(&url).await.unwrap());
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let owner = UserId::new(Uuid::new_v4().to_string());
    store
        .save_user(&User {
            id: owner.clone(),
            email: format!("{}@rss-runner.example", owner.as_str()),
            display_name: "RSS runner".into(),
        })
        .await
        .unwrap();
    let mut ids = Vec::new();
    for _ in 0..2 {
        let sub = store
            .create_subscription(
                &owner,
                &Uuid::new_v4().to_string(),
                &SubscriptionInput {
                    name: "rss".into(),
                    source_url: "https://example.org/private-feed".into(),
                    enabled: true,
                },
            )
            .await
            .unwrap();
        let now: i64 =
            sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
                .fetch_one(&pool)
                .await
                .unwrap();
        let saved = store
            .preview_feed_schedule(
                &owner,
                &sub.snapshot.subscription_id,
                &ScheduleInput {
                    schedule_id: Uuid::new_v4().to_string(),
                    starts_at_unix_ms: u64::try_from(now + 600_000).unwrap(),
                    ends_at_unix_ms: u64::try_from(now + 86_400_000).unwrap(),
                    interval_hours: 1,
                },
            )
            .await
            .unwrap();
        store
            .approve_feed_schedule(&owner, &saved.plan.input.schedule_id, &saved.digest)
            .await
            .unwrap();
        let mut plan = saved.plan;
        let delta = 630_000_u64;
        plan.created_at_unix_ms -= delta;
        plan.approval_expires_at_unix_ms -= delta;
        plan.input.starts_at_unix_ms -= delta;
        plan.input.ends_at_unix_ms -= delta;
        sqlx::query("UPDATE feed_schedules SET plan=$3,digest=$4,created_ms=$5,approval_expires_ms=$6,ends_ms=$7,approved_ms=approved_ms-$8 WHERE user_id=$1 AND id=$2")
            .bind(Uuid::parse_str(owner.as_str()).unwrap()).bind(Uuid::parse_str(&plan.input.schedule_id).unwrap()).bind(serde_json::to_value(&plan).unwrap()).bind(plan.consent_digest().unwrap())
            .bind(i64::try_from(plan.created_at_unix_ms).unwrap()).bind(i64::try_from(plan.approval_expires_at_unix_ms).unwrap()).bind(i64::try_from(plan.input.ends_at_unix_ms).unwrap()).bind(i64::try_from(delta).unwrap()).execute(&pool).await.unwrap();
        ids.push(plan.input.schedule_id);
    }
    assert!(super::run(Some("local"), &url).status.success());
    assert!(
        store
            .list_collections(&owner, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let abandoned = store
        .claim_scheduled_collection(&owner, &ids[0])
        .await
        .unwrap();
    sqlx::query("UPDATE feed_collections SET claimed_ms=claimed_ms-120000,deadline_ms=deadline_ms-120000 WHERE user_id=$1").bind(Uuid::parse_str(owner.as_str()).unwrap()).execute(&pool).await.unwrap();
    let transport = Arc::new(Transport(AtomicUsize::new(0)));
    let mut runner = scheduler::FeedScheduleRunner::new(store.clone(), transport.clone());
    let result = runner.tick().await.unwrap();
    assert_eq!(result.recovered, 1);
    assert_eq!(result.completed, 1);
    assert_eq!(
        store
            .get_collection(&owner, &abandoned.request_id)
            .await
            .unwrap()
            .status,
        CollectionStatus::Unknown
    );
    assert!(
        store
            .list_collections(&owner, None)
            .await
            .unwrap()
            .items
            .iter()
            .any(|item| item.status == CollectionStatus::Succeeded)
    );
    assert_eq!(transport.0.load(Ordering::SeqCst), 1);
    let mut restarted = scheduler::FeedScheduleRunner::new(store.clone(), transport.clone());
    let result = restarted.tick().await.unwrap();
    assert_eq!(result.completed, 0);
    assert_eq!(result.recovered, 0);
    assert_eq!(transport.0.load(Ordering::SeqCst), 1);
    // 所有时段已消费后启动真实 enabled 进程，验证接线且不访问外部服务。
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_scheduler"))
        .env("SCHEDULER_MODE", "local")
        .env("RSS_SCHEDULES_ENABLED", "true")
        .env_remove("RUN_FOREVER")
        .env("DATABASE_URL", &url)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("RSS schedules:"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-feed"));
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
}
