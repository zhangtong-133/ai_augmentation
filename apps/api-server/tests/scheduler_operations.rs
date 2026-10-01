use personal_ai_domain::{User, UserId};
use personal_ai_storage::{MetadataStore, schedule_operations::ScheduleOperationsStore};
use personal_ai_storage_postgres::PostgresStore;
use serde_json::Value;
use sqlx::ConnectOptions;
use std::{
    process::{Command, Output},
    str::FromStr,
};
use uuid::Uuid;

fn cli(url: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_scheduler-operations"))
        .args(args)
        .env("DATABASE_URL", url)
        .env("SCHEDULER_MODE", "local")
        .env("RUN_FOREVER", "1")
        .env("MODEL_AGENT_OPENAI_API_KEY", "private-operations-key")
        .output()
        .unwrap()
}
fn json(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("private-title"));
    assert!(!stdout.contains("private-body"));
    assert!(!stdout.contains("private-operations-key"));
    assert!(!stdout.contains("claim_id"));
    assert!(!stdout.contains("digest"));
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stderr)))
}
struct Fixture {
    store: PostgresStore,
    pool: sqlx::PgPool,
    owner: UserId,
    url: String,
    now: i64,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let store = PostgresStore::connect(&url).await.unwrap();
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let owner = UserId::new(Uuid::new_v4().to_string());
        store
            .save_user(&User {
                id: owner.clone(),
                email: format!("{}@ops.example", owner.as_str()),
                display_name: "运维测试".into(),
            })
            .await
            .unwrap();
        let now =
            sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
                .fetch_one(&pool)
                .await
                .unwrap();
        Self {
            store,
            pool,
            owner,
            url,
            now,
        }
    }
    async fn seed(&self, status: &str, due: bool, expired_lease: bool) -> String {
        let request = Uuid::new_v4();
        let run_at = if due {
            self.now - 60000
        } else {
            self.now + 3_600_000
        };
        let approved =
            matches!(status, "scheduled" | "running" | "delivered").then_some(self.now - 3_600_000);
        let claim = matches!(status, "running" | "delivered").then(Uuid::new_v4);
        let lease = claim.map(|_| {
            if expired_lease {
                self.now - 1000
            } else {
                self.now + 600_000
            }
        });
        sqlx::query("INSERT INTO schedules(user_id,request_id,version,title,body,run_at_ms,digest,status,created_ms,approval_expires_ms,approved_ms,approval,claim_id,lease_until_ms,delivered_ms,cancelled_ms) VALUES($1,$2,'local-reminder-once-v1','private-title','private-body',$3,repeat('a',64),$4,$5,$6,$7,$8,$9,$10,$11,$12)")
            .bind(Uuid::parse_str(self.owner.as_str()).unwrap()).bind(request).bind(run_at).bind(status).bind(self.now-7_200_000).bind(run_at).bind(approved).bind(approved.map(|_|serde_json::json!({}))).bind(claim).bind(lease).bind((status=="delivered").then_some(self.now)).bind((status=="cancelled").then_some(self.now)).execute(&self.pool).await.unwrap();
        request.to_string()
    }
    async fn reminder(&self, request: &str, time: i64) {
        sqlx::query("INSERT INTO schedule_reminders(user_id,request_id,title,body,delivered_ms) VALUES($1,$2,'private-title','private-body',$3)")
            .bind(Uuid::parse_str(self.owner.as_str()).unwrap()).bind(Uuid::parse_str(request).unwrap()).bind(time).execute(&self.pool).await.unwrap();
    }
    async fn cleanup(&self) {
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(Uuid::parse_str(self.owner.as_str()).unwrap())
            .execute(&self.pool)
            .await
            .unwrap();
    }
    fn args(&self) -> [&str; 3] {
        ["audit", "--user", self.owner.as_str()]
    }
}
async fn readonly(f: &Fixture) -> (String, String) {
    let role = format!("schedule_ops_{}", Uuid::new_v4().simple());
    let password = Uuid::new_v4().simple().to_string();
    sqlx::raw_sql(&format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'; ALTER ROLE {role} SET default_transaction_read_only=on;
        GRANT SELECT (id) ON users TO {role};
        GRANT SELECT (user_id,request_id,version,status,run_at_ms,created_ms,approval_expires_ms,approved_ms,lease_until_ms,delivered_ms,cancelled_ms) ON schedules TO {role};
        GRANT SELECT (user_id,request_id,delivered_ms) ON schedule_reminders TO {role}")).execute(&f.pool).await.unwrap();
    let options = sqlx::postgres::PgConnectOptions::from_str(&f.url)
        .unwrap()
        .username(&role)
        .password(&password);
    (role, options.to_url_lossy().to_string())
}
async fn drop_role(f: &Fixture, role: &str) {
    sqlx::raw_sql(&format!("DROP OWNED BY {role}; DROP ROLE {role}"))
        .execute(&f.pool)
        .await
        .unwrap();
}

#[test]
fn invalid_arguments_fail_before_connecting_without_exposing_credentials() {
    let output = cli(
        "postgres://private-password@invalid/db",
        &["audit", "--user", "invalid"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-password"));
    assert!(cli("invalid", &["--help"]).status.success());
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn scheduler_audit_supports_column_only_readonly_roles_and_classifies_backlog() {
    let f = Fixture::new().await;
    let other = Fixture::new().await;
    let (role, url) = readonly(&f).await;
    let empty = json(&cli(&url, &f.args()));
    assert_eq!(empty["counts"]["tasks"], 0);
    assert!(empty["oldest_ready_delay_ms"].is_null());
    f.seed("draft", false, false).await;
    f.seed("draft", true, false).await;
    f.seed("scheduled", false, false).await;
    f.seed("scheduled", true, false).await;
    f.seed("running", true, false).await;
    f.seed("running", true, true).await;
    let delivered = f.seed("delivered", true, false).await;
    f.reminder(&delivered, f.now).await;
    f.seed("cancelled", true, false).await;
    f.seed("failed", true, false).await;
    other.seed("scheduled", true, false).await;
    let output = cli(&url, &f.args());
    assert!(output.status.success());
    let report = json(&output);
    assert_eq!(
        report["counts"],
        serde_json::json!({"tasks":9,"drafts":1,"expired_drafts":1,"scheduled":2,"due":1,"running":2,"expired_leases":1,"delivered":1,"cancelled":1,"failed":1,"reminders":1,"inconsistent":0})
    );
    assert_eq!(report["ready_to_claim"], 2);
    assert_eq!(report["worker_liveness"], "unknown");
    assert_eq!(report["consistent"], true);
    assert!(
        report["oldest_ready_delay_ms"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap()
            >= 60000
    );
    assert!(report["items"][0]["run_at_unix_ms"].is_string());
    let missing = cli(&url, &["audit", "--user", &Uuid::new_v4().to_string()]);
    assert_eq!(missing.status.code(), Some(1));
    drop_role(&f, &role).await;
    other.cleanup().await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn scheduler_audit_detects_missing_unexpected_and_mismatched_reminders_without_repair() {
    let f = Fixture::new().await;
    let missing = f.seed("delivered", true, false).await;
    let mismatch = f.seed("delivered", true, false).await;
    f.reminder(&mismatch, f.now + 1).await;
    let unexpected = f.seed("cancelled", true, false).await;
    f.reminder(&unexpected, f.now).await;
    let approved = f.seed("scheduled", true, false).await;
    sqlx::query(
        "UPDATE schedules SET approved_ms=approval_expires_ms WHERE user_id=$1 AND request_id=$2",
    )
    .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
    .bind(Uuid::parse_str(&approved).unwrap())
    .execute(&f.pool)
    .await
    .unwrap();
    let (role, url) = readonly(&f).await;
    let output = cli(&url, &f.args());
    assert_eq!(output.status.code(), Some(2));
    let report = json(&output);
    assert_eq!(report["counts"]["inconsistent"], 4);
    for (id, issue) in [
        (&missing, "missing_reminder"),
        (&mismatch, "delivery_time_mismatch"),
        (&unexpected, "unexpected_reminder"),
        (&approved, "approval_outside_window"),
    ] {
        let item = report["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["request_id"] == *id)
            .unwrap();
        assert!(
            item["issues"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!(issue))
        );
    }
    let second = json(&cli(&url, &f.args()));
    assert_eq!(report["items"], second["items"]);
    drop_role(&f, &role).await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn scheduler_audit_paging_keeps_full_totals_and_metadata_exact() {
    let f = Fixture::new().await;
    let mut ids = Vec::new();
    for _ in 0..105 {
        ids.push(f.seed("scheduled", true, false).await);
    }
    ids.sort();
    // 故障数据不应因位于下一页而漏报；大整数保持精确。
    sqlx::query(
        "UPDATE schedules SET lease_until_ms=$3,claim_id=$4 WHERE user_id=$1 AND request_id=$2",
    )
    .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
    .bind(Uuid::parse_str(ids.last().unwrap()).unwrap())
    .bind(i64::MAX)
    .bind(Uuid::new_v4())
    .execute(&f.pool)
    .await
    .unwrap();
    let first = cli(&f.url, &f.args());
    assert_eq!(first.status.code(), Some(2));
    let first = json(&first);
    assert_eq!(first["counts"]["tasks"], 105);
    assert_eq!(first["counts"]["inconsistent"], 1);
    assert_eq!(first["items"].as_array().unwrap().len(), 100);
    let second = json(&cli(
        &f.url,
        &[
            "audit",
            "--user",
            f.owner.as_str(),
            "--after",
            first["next_cursor"].as_str().unwrap(),
        ],
    ));
    assert_eq!(second["counts"]["tasks"], 105);
    assert_eq!(second["items"].as_array().unwrap().len(), 5);
    assert!(second["next_cursor"].is_null());
    let seen: Vec<_> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["items"].as_array().unwrap())
        .map(|i| i["request_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(seen, ids);
    assert_eq!(
        second["items"][4]["lease_until_unix_ms"],
        i64::MAX.to_string()
    );
    assert!(
        f.store
            .audit_schedules(&f.owner, Some("bad-cursor"))
            .await
            .is_err()
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn scheduler_audit_snapshot_stays_consistent_during_atomic_deliveries() {
    let f = Fixture::new().await;
    let mut ids = Vec::new();
    for _ in 0..20 {
        ids.push(f.seed("scheduled", true, false).await);
    }
    let owner = Uuid::parse_str(f.owner.as_str()).unwrap();
    let writer = async {
        for id in ids {
            let request = Uuid::parse_str(&id).unwrap();
            let mut tx = f.pool.begin().await.unwrap();
            sqlx::query("UPDATE schedules SET status='delivered',delivered_ms=$3,claim_id=$4,lease_until_ms=$3+60000 WHERE user_id=$1 AND request_id=$2")
                .bind(owner).bind(request).bind(f.now).bind(Uuid::new_v4()).execute(&mut *tx).await.unwrap();
            tokio::task::yield_now().await;
            sqlx::query("INSERT INTO schedule_reminders(user_id,request_id,title,body,delivered_ms) VALUES($1,$2,'private-title','private-body',$3)")
                .bind(owner).bind(request).bind(f.now).execute(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
        }
    };
    let reader = async {
        for _ in 0..30 {
            let report = f.store.audit_schedules(&f.owner, None).await.unwrap();
            assert!(report.consistent);
            assert_eq!(report.counts.tasks, 20);
            assert_eq!(report.counts.delivered, report.counts.reminders);
            assert_eq!(
                report.counts.delivered,
                i64::try_from(report.items.iter().filter(|i| i.reminder_present).count()).unwrap()
            );
        }
    };
    tokio::join!(writer, reader);
    assert_eq!(
        f.store
            .audit_schedules(&f.owner, None)
            .await
            .unwrap()
            .counts
            .delivered,
        20
    );
    f.cleanup().await;
}
