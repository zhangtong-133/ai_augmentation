use std::process::{Command, Output};

fn run(mode: Option<&str>, url: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_scheduler"));
    command
        .env_remove("SCHEDULER_MODE")
        .env_remove("RUN_FOREVER")
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
async fn local_scheduler_process_delivers_once_and_survives_restart() {
    use personal_ai_agent_core::schedules::schedule_digest;
    use personal_ai_domain::{User, UserId};
    use personal_ai_storage::{
        MetadataStore,
        schedules::{
            NewSchedule, SCHEDULE_VERSION, ScheduleApproval, ScheduleDeliveryStore, ScheduleStore,
        },
    };
    use personal_ai_storage_postgres::PostgresStore;
    use uuid::Uuid;
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
    assert!(run(None, &url).status.success());
    assert!(
        store
            .list_schedule_reminders(&owner, None)
            .await
            .unwrap()
            .items
            .is_empty()
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
