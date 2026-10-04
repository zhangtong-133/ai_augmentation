use personal_ai_domain::{User, UserId};
use personal_ai_storage::{
    MetadataStore,
    feeds::{FeedStore, SubscriptionInput},
};
use personal_ai_storage_postgres::PostgresStore;
use sqlx::{ConnectOptions, postgres::PgConnectOptions};
use std::{process::Command, str::FromStr};
use uuid::Uuid;

#[tokio::test]
#[ignore = "需要有建角色权限的一次性 TEST_DATABASE_URL"]
async fn metadata_only_cli_has_stable_exit_codes_and_never_reads_private_columns() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let owner = UserId::new(Uuid::new_v4().to_string());
    store
        .save_user(&User {
            id: owner.clone(),
            email: format!("{owner}@ops.example"),
            display_name: "private".into(),
        })
        .await
        .unwrap();
    let sub = Uuid::new_v4().to_string();
    store
        .create_subscription(
            &owner,
            &sub,
            &SubscriptionInput {
                name: "private".into(),
                source_url: "https://example.com/rss?secret=private".into(),
                enabled: true,
            },
        )
        .await
        .unwrap();
    let draft = store
        .preview_collection(&owner, &sub, &Uuid::new_v4().to_string())
        .await
        .unwrap();
    let role = format!("rss_ops_{}", Uuid::new_v4().simple());
    let password = Uuid::new_v4().simple().to_string();
    grant_metadata(&pool, &role, &password).await;
    let limited = PgConnectOptions::from_str(&url)
        .unwrap()
        .username(&role)
        .password(&password)
        .to_url_lossy()
        .to_string();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_feed-operations"))
            .args(args)
            .env("DATABASE_URL", &limited)
            .output()
            .unwrap()
    };
    let result = run(&["audit", "--user", owner.as_str()]);
    assert_eq!(result.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["counts"]["collections"], 1);
    assert_eq!(report["consistent"], true);
    assert_eq!(result.stderr, [] as [u8; 0]);
    let reader = sqlx::PgPool::connect(&limited).await.unwrap();
    assert!(
        sqlx::query("SELECT plan FROM feed_collections")
            .fetch_all(&reader)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE feed_collections SET status='draft'")
            .execute(&reader)
            .await
            .is_err()
    );
    reader.close().await;
    sqlx::query("DELETE FROM feed_collection_audit WHERE user_id=$1 AND request_id=$2")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .bind(Uuid::parse_str(&draft.plan.request_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        run(&["audit", "--user", owner.as_str()]).status.code(),
        Some(2)
    );
    assert_eq!(
        run(&["audit", "--user", &Uuid::new_v4().to_string()])
            .status
            .code(),
        Some(1)
    );
    assert_eq!(
        run(&["audit", "--user", owner.as_str(), "--apply"])
            .status
            .code(),
        Some(1)
    );
    for statement in [format!("DROP OWNED BY {role}"), format!("DROP ROLE {role}")] {
        sqlx::query(&statement).execute(&pool).await.unwrap();
    }
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
}

async fn grant_metadata(pool: &sqlx::PgPool, role: &str, password: &str) {
    for statement in [
        format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'"),
        format!("GRANT USAGE ON SCHEMA public TO {role}"),
        format!("GRANT SELECT(id) ON users TO {role}"),
        format!("GRANT SELECT(user_id,id,enabled,deleted) ON feed_subscriptions TO {role}"),
        format!(
            "GRANT SELECT(user_id,request_id,subscription_id,status,created_ms,claimed_ms,deadline_ms,finished_ms,reason,inserted,updated,unchanged) ON feed_collections TO {role}"
        ),
        format!(
            "GRANT SELECT(user_id,subscription_id,first_seen_ms,updated_ms,last_seen_ms) ON feed_entries TO {role}"
        ),
        format!("GRANT SELECT ON feed_collection_audit TO {role}"),
    ] {
        sqlx::query(&statement).execute(pool).await.unwrap();
    }
}
#[tokio::test]
#[ignore = "需要有建角色权限的一次性 TEST_DATABASE_URL"]
async fn value_audit_uses_only_metadata_and_preserves_expired_requests() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let owner = UserId::new(Uuid::new_v4().to_string());
    store
        .save_user(&User {
            id: owner.clone(),
            email: format!("{owner}@ops.example"),
            display_name: "private".into(),
        })
        .await
        .unwrap();
    let id = Uuid::new_v4();
    let uid = Uuid::parse_str(owner.as_str()).unwrap();
    sqlx::query("INSERT INTO feed_value_reviews(user_id,id,status,snapshot,pricing,digest,created_ms,expires_ms) SELECT $1,$2,'draft','{\"private\":\"hidden-body\"}','{\"private\":\"hidden-target\"}',repeat('a',64),t-600000,t-300000 FROM (SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint t) clock")
        .bind(uid).bind(id).execute(&pool).await.unwrap();
    let role = format!("value_ops_{}", Uuid::new_v4().simple());
    let password = Uuid::new_v4().simple().to_string();
    grant_value_metadata(&pool, &role, &password).await;
    let limited = PgConnectOptions::from_str(&url)
        .unwrap()
        .username(&role)
        .password(&password)
        .to_url_lossy()
        .to_string();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_feed-operations"))
            .args(args)
            .env("DATABASE_URL", &limited)
            .output()
            .unwrap()
    };
    let report = run(&["audit-values", "--user", owner.as_str()]);
    assert_eq!(report.status.code(), Some(0));
    let saved: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(saved["counts"]["records"], 1);
    assert_eq!(saved["warnings"], serde_json::json!(["expired_active"]));
    assert_eq!(saved["remaining_records"], 999);
    assert!(!String::from_utf8_lossy(&report.stdout).contains("hidden"));
    let reader = sqlx::PgPool::connect(&limited).await.unwrap();
    for column in ["snapshot", "pricing", "scores", "dispatch_token"] {
        assert!(
            sqlx::query(&format!("SELECT {column} FROM feed_value_reviews"))
                .fetch_all(&reader)
                .await
                .is_err()
        );
    }
    assert!(
        sqlx::query("UPDATE feed_value_reviews SET status='expired',snapshot=NULL")
            .execute(&reader)
            .await
            .is_err()
    );
    reader.close().await;
    let status: String =
        sqlx::query_scalar("SELECT status FROM feed_value_reviews WHERE user_id=$1 AND id=$2")
            .bind(uid)
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "draft");
    sqlx::query("DELETE FROM feed_value_audit WHERE user_id=$1")
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        run(&["audit-values", "--user", owner.as_str()])
            .status
            .code(),
        Some(2)
    );
    for args in [
        vec!["audit-values", "--user", owner.as_str(), "--apply"],
        vec!["audit-values", "--user", owner.as_str(), "--after", "bad"],
    ] {
        assert_eq!(run(&args).status.code(), Some(1));
    }
    assert_eq!(
        run(&["audit-values", "--user", &Uuid::new_v4().to_string()])
            .status
            .code(),
        Some(1)
    );
    for statement in [format!("DROP OWNED BY {role}"), format!("DROP ROLE {role}")] {
        sqlx::query(&statement).execute(&pool).await.unwrap();
    }
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
}

async fn grant_value_metadata(pool: &sqlx::PgPool, role: &str, password: &str) {
    for statement in [
        format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'"),
        format!("GRANT USAGE ON SCHEMA public TO {role}"),
        format!("GRANT SELECT(id) ON users TO {role}"),
        format!(
            "GRANT SELECT(user_id,id,status,created_ms,expires_ms,approved_ms,dispatch_deadline_ms,sent_ms) ON feed_value_reviews TO {role}"
        ),
        format!(
            "GRANT SELECT(user_id,request_id,sequence,event,at_ms) ON feed_value_audit TO {role}"
        ),
    ] {
        sqlx::query(&statement).execute(pool).await.unwrap();
    }
}
