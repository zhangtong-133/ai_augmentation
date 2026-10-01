use personal_ai_domain::{User, UserId};
use personal_ai_storage::{
    MetadataStore,
    conversations::ConversationStore,
    messages::MessageStore,
    replies::ReplyConfiguration,
    reply_budgets::{ReplyBudget, ReplyDispatchStore},
};
use personal_ai_storage_postgres::PostgresStore;
use serde_json::Value;
use sqlx::ConnectOptions;
use std::{
    process::{Command, Output},
    str::FromStr,
};
use uuid::Uuid;

fn cli(url: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_reply-operations"))
        .args(args)
        .env("DATABASE_URL", url)
        // 管理命令不能受模型开关或供应商配置影响。
        .env("CONVERSATION_REPLY_MODE", "openai")
        .env("REPLY_OPENAI_API_KEY", "local-test-key-not-for-output")
        .output()
        .unwrap()
}

fn json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(!text.contains("local-test-key-not-for-output"));
    assert!(!text.contains("not-in-admin-json"));
    serde_json::from_str(&text).unwrap()
}

async fn readonly_role(pool: &sqlx::PgPool, url: &str) -> (String, String) {
    let role = format!("reply_ops_{}", Uuid::new_v4().simple());
    let password = Uuid::new_v4().simple().to_string();
    sqlx::query(&format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(&format!(
        "ALTER ROLE {role} SET default_transaction_read_only=on"
    ))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(&format!("GRANT SELECT ON users,conversations,conversation_replies,reply_configurations,reply_money_daily,reply_money_reservations TO {role}"))
        .execute(pool).await.unwrap();
    sqlx::query(&format!("GRANT SELECT (user_id,conversation_id,request_id,status) ON model_execution_call_audit TO {role}"))
        .execute(pool).await.unwrap();
    sqlx::query(&format!("GRANT SELECT (user_id,conversation_id,request_id,status) ON model_planning_requests TO {role}"))
        .execute(pool).await.unwrap();
    let options = sqlx::postgres::PgConnectOptions::from_str(url)
        .unwrap()
        .username(&role)
        .password(&password);
    (role, options.to_url_lossy().to_string())
}

async fn drop_role(pool: &sqlx::PgPool, role: &str) {
    sqlx::query(&format!("DROP OWNED BY {role}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(&format!("DROP ROLE {role}"))
        .execute(pool)
        .await
        .unwrap();
}

#[cfg(unix)]
#[test]
fn invalid_database_environment_does_not_print_credentials() {
    use std::os::unix::ffi::OsStrExt;
    let url =
        std::ffi::OsStr::from_bytes(b"postgres://user:private-test-password\xff@localhost/db");
    let output = Command::new(env!("CARGO_BIN_EXE_reply-operations"))
        .args(["configuration", "v1"])
        .env("DATABASE_URL", url)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-test-password"));
    assert_eq!(output.stdout, [] as [u8; 0]);
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn configuration_cli_supports_readonly_queries_and_requires_explicit_disable() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let revision = Uuid::new_v4().to_string();
    let configuration = ReplyConfiguration {
        model: "local-fixture".into(),
        revision: revision.clone(),
    };
    let budget = ReplyBudget {
        currency: "USD".into(),
        provider: "local-fixture".into(),
        price_version: "price-v1".into(),
        counter_version: "counter-v1".into(),
        input_price_per_million: u64::MAX,
        output_price_per_million: u64::MAX,
        input_token_bound: 100,
        output_token_bound: 1024,
        request_limit: i64::MAX,
        daily_limit: i64::MAX,
    };
    store
        .register_reply_configuration(&configuration, &budget, 4_102_444_800_000)
        .await
        .unwrap();
    let (role, readonly_url) = readonly_role(&pool, &url).await;
    let detail = json(&cli(&readonly_url, &["configuration", &revision]));
    assert_eq!(detail["active"], true);
    assert_eq!(
        detail["budget"]["input_price_micro_per_million"],
        u64::MAX.to_string()
    );
    assert_eq!(detail["budget"]["daily_limit_micro"], i64::MAX.to_string());
    assert!(json(&cli(&readonly_url, &["configurations"]))["items"].is_array());
    let preview = json(&cli(&readonly_url, &["disable", &revision]));
    assert_eq!(preview["apply"], false);
    assert_eq!(preview["configuration"]["active"], true);
    let denied = cli(&readonly_url, &["disable", &revision, "--apply"]);
    assert_eq!(denied.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&denied.stderr).contains(&readonly_url));
    assert!(
        store
            .check_reply_configuration(&configuration, &budget)
            .await
            .is_ok()
    );
    let applied = json(&cli(&url, &["disable", &revision, "--apply"]));
    assert_eq!(applied["apply"], true);
    assert_eq!(applied["configuration"]["active"], false);
    let repeated = json(&cli(&url, &["disable", &revision, "--apply"]));
    assert_eq!(
        applied["configuration"]["disabled_at_unix_ms"],
        repeated["configuration"]["disabled_at_unix_ms"]
    );
    assert!(
        store
            .check_reply_configuration(&configuration, &budget)
            .await
            .is_err()
    );
    drop_role(&pool, &role).await;
    sqlx::query("DELETE FROM reply_configurations WHERE revision=$1")
        .bind(revision)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn ledger_cli_is_readonly_exact_and_returns_nonzero_for_mismatch() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let user = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@operations.example", Uuid::new_v4()),
        display_name: "管理员验收".into(),
    };
    store.save_user(&user).await.unwrap();
    let conversation = store
        .create_conversation(&user.id, &Uuid::new_v4().to_string(), "not-in-admin-json")
        .await
        .unwrap()
        .id;
    store
        .append_message(
            &user.id,
            &conversation,
            &Uuid::new_v4().to_string(),
            "not-in-admin-json",
        )
        .await
        .unwrap();
    let owner = Uuid::parse_str(user.id.as_str()).unwrap();
    sqlx::query("INSERT INTO reply_money_daily(user_id,day,currency,occupied) VALUES($1,'2030-01-01','USD',$2)")
        .bind(owner).bind(i64::MAX).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO reply_money_reservations(user_id,conversation_id,request_id,day,currency,model,configuration_revision,budget,reserved,charged,settlement,settled_at) VALUES($1,$2,$3,'2030-01-01','USD','local-fixture','v1','{}'::jsonb,$4,$4,'retained',clock_timestamp())")
        .bind(owner).bind(Uuid::parse_str(&conversation).unwrap()).bind(Uuid::new_v4()).bind(i64::MAX).execute(&pool).await.unwrap();
    let (role, readonly_url) = readonly_role(&pool, &url).await;
    let args = [
        "ledger",
        "--user",
        user.id.as_str(),
        "--day",
        "2030-01-01",
        "--currency",
        "USD",
    ];
    let report = json(&cli(&readonly_url, &args));
    assert_eq!(report["consistent"], true);
    assert_eq!(report["ledger_occupied_micro"], i64::MAX.to_string());
    assert_eq!(report["totals"]["retained_micro"], i64::MAX.to_string());
    assert!(report["items"][0]["context"].is_null());
    sqlx::query("UPDATE reply_money_daily SET occupied=occupied-1 WHERE user_id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    let mismatch = cli(&readonly_url, &args);
    assert_eq!(mismatch.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&mismatch.stdout).unwrap();
    assert_eq!(report["consistent"], false);
    assert_eq!(report["difference_micro"], "-1");
    let occupied: i64 =
        sqlx::query_scalar("SELECT occupied FROM reply_money_daily WHERE user_id=$1")
            .bind(owner)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(occupied, i64::MAX - 1);
    drop_role(&pool, &role).await;
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
}
