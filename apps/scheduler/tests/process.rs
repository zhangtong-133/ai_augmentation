use std::process::{Command, Output};

// 真实 scheduler 扫描所有用户；共享数据库的进程测试必须互斥，避免相互领取夹具。
static PROCESS_DATABASE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn run(mode: Option<&str>, url: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_scheduler"));
    command
        .env_remove("SCHEDULER_MODE")
        .env_remove("RUN_FOREVER")
        .env_remove("RSS_SCHEDULES_ENABLED")
        .env("DATABASE_URL", url);
    if let Some(mode) = mode {
        command.env("SCHEDULER_MODE", mode);
    }
    command.output().unwrap()
}

#[test]
fn disabled_scheduler_does_not_connect_and_invalid_mode_is_rejected() {
    for mode in [None, Some("disabled")] {
        let output = run(mode, "private-invalid-url");
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("disabled"));
    }
    let invalid = run(Some("openai"), "private-invalid-url");
    assert!(!invalid.status.success());
    assert!(!String::from_utf8_lossy(&invalid.stderr).contains("private-invalid-url"));
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)] // 同一进程生命周期验证提醒和定时日报的禁用、投递及重启幂等。
async fn local_scheduler_process_delivers_once_and_survives_restart() {
    use personal_ai_agent_core::schedules::schedule_digest;
    use personal_ai_domain::{User, UserId};
    use personal_ai_storage::{
        MetadataStore,
        brief_schedules::BriefScheduleStore,
        briefs::BriefStore,
        schedules::{
            NewSchedule, SCHEDULE_VERSION, ScheduleApproval, ScheduleDeliveryStore, ScheduleStore,
        },
    };
    use personal_ai_storage_postgres::PostgresStore;
    use uuid::Uuid;
    let _database_guard = PROCESS_DATABASE.lock().await;
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let owner = UserId::new(Uuid::new_v4().to_string());
    store
        .save_user(&User {
            id: owner.clone(),
            email: format!("{}@scheduler.example", owner.as_str()),
            display_name: "后台进程验收".into(),
        })
        .await
        .unwrap();
    let now: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
            .fetch_one(&pool)
            .await
            .unwrap();
    let input = NewSchedule {
        request_id: Uuid::new_v4().to_string(),
        title: "private-reminder-title".into(),
        body: "private-reminder-body".into(),
        run_at_unix_ms: now - 1000,
    };
    let digest = schedule_digest(&owner, &input);
    let approval = ScheduleApproval {
        digest: digest.clone(),
        accepted_run_at_unix_ms: input.run_at_unix_ms,
        accepted_max_runs: 1,
        accepted_amount_micro: 0,
        acknowledge_schedule: true,
    };
    sqlx::query("INSERT INTO schedules(user_id,request_id,version,title,body,run_at_ms,digest,status,created_ms,approval_expires_ms,approved_ms,approval) VALUES($1,$2,$3,$4,$5,$6,$7,'scheduled',$6-3600000,$6-1800000,$6-3500000,$8)")
        .bind(Uuid::parse_str(owner.as_str()).unwrap()).bind(Uuid::parse_str(&input.request_id).unwrap()).bind(SCHEDULE_VERSION).bind(&input.title).bind(&input.body).bind(input.run_at_unix_ms).bind(digest).bind(serde_json::to_value(approval).unwrap()).execute(&pool).await.unwrap();
    store.save_brief_schedule(&owner, 0, true, 0).await.unwrap();
    sqlx::query("UPDATE feed_brief_schedules SET next_run_ms=0 WHERE user_id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    assert!(run(None, &url).status.success());
    assert_eq!(
        store.list_briefs(&owner, None).await.unwrap().items.len(),
        0
    );
    assert_eq!(
        store
            .list_schedule_reminders(&owner, None)
            .await
            .unwrap()
            .items,
        [] as [personal_ai_storage::schedules::ScheduleReminder; 0]
    );
    for _ in 0..2 {
        let output = run(Some("local"), &url);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for text in [&output.stdout, &output.stderr] {
            assert!(!String::from_utf8_lossy(text).contains("private-reminder"));
            assert!(!String::from_utf8_lossy(text).contains(&url));
        }
        assert_eq!(
            store.list_briefs(&owner, None).await.unwrap().items.len(),
            1
        );
        assert_eq!(
            store
                .get_brief_schedule(&owner)
                .await
                .unwrap()
                .last_outcome
                .as_deref(),
            Some("generated")
        );
        let reminders = store.list_schedule_reminders(&owner, None).await.unwrap();
        assert_eq!(reminders.items.len(), 1);
        assert_eq!(reminders.items[0].body, input.body);
        assert_eq!(
            store
                .get_schedule(&owner, &input.request_id)
                .await
                .unwrap()
                .status,
            "delivered"
        );
    }
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
}

#[test]
fn invalid_rss_switch_is_rejected_before_database_access() {
    let output = Command::new(env!("CARGO_BIN_EXE_scheduler"))
        .env("SCHEDULER_MODE", "local")
        .env("RSS_SCHEDULES_ENABLED", "yes")
        .env("DATABASE_URL", "private-invalid-url")
        .env_remove("RUN_FOREVER")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("RSS_SCHEDULES_ENABLED"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-invalid-url"));
}

#[path = "process/rss_runner.rs"]
mod rss_runner;

#[test]
fn disabled_scheduler_overrides_the_rss_switch() {
    let output = Command::new(env!("CARGO_BIN_EXE_scheduler"))
        .env("SCHEDULER_MODE", "disabled")
        .env("RSS_SCHEDULES_ENABLED", "true")
        .env_remove("RUN_FOREVER")
        .env("DATABASE_URL", "private-invalid-url")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("scheduler disabled"));
}
