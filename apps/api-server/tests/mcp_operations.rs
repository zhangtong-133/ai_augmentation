use personal_ai_domain::{User, UserId};
use personal_ai_storage::{MetadataStore, mcp_operations::McpOperationsStore};
use personal_ai_storage_postgres::PostgresStore;
use sqlx::{ConnectOptions, Row};
use std::{process::Command, str::FromStr};
use uuid::Uuid;

async fn fixture() -> (String, PostgresStore, sqlx::PgPool, UserId) {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let owner = UserId::new(Uuid::new_v4().to_string());
    store
        .save_user(&User {
            id: owner.clone(),
            email: format!("{}@mcp-ops.example", owner.as_str()),
            display_name: "private-owner".into(),
        })
        .await
        .unwrap();
    (url, store, pool, owner)
}
async fn seed(
    pool: &sqlx::PgPool,
    owner: &UserId,
    count: i32,
    created: &str,
    expires: &str,
    revoked: Option<&str>,
) {
    sqlx::query("INSERT INTO mcp_credentials(id,user_id,token_digest,host_name,created_at,expires_at,revoked_at)
        SELECT gen_random_uuid(),$1,replace(gen_random_uuid()::text,'-','')||replace(gen_random_uuid()::text,'-',''),'private-host',NOW()+$3::interval,NOW()+$4::interval,NOW()+$5::interval FROM generate_series(1,$2)")
        .bind(Uuid::parse_str(owner.as_str()).unwrap()).bind(count).bind(created).bind(expires).bind(revoked).execute(pool).await.unwrap();
}
async fn cleanup(pool: &sqlx::PgPool, owner: &UserId) {
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .execute(pool)
        .await
        .unwrap();
}
fn cli(url: &str, owner: &UserId) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mcp-operations"))
        .args(["audit", "--user", owner.as_str()])
        .env("DATABASE_URL", url)
        .env("MCP_ACCESS_TOKEN", "private-access-token")
        .env("MCP_ALLOW_EMBEDDING_COST", "1")
        .output()
        .unwrap()
}
#[test]
fn invalid_arguments_fail_without_connecting_or_printing_secrets() {
    let output = cli(
        "postgres://private-password@invalid/db",
        &UserId::new("bad"),
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, [] as [u8; 0]);
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private-password"));
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn mcp_audit_is_paginated_private_and_works_with_column_only_read_permissions() {
    let (url, store, pool, owner) = fixture().await;
    seed(&pool, &owner, 105, "-40 days", "-30 days", None).await;
    seed(&pool, &owner, 2, "-2 days", "1 day", None).await;
    seed(&pool, &owner, 1, "-1 hour", "1 day", None).await;
    seed(&pool, &owner, 1, "-1 hour", "1 day", Some("0 seconds")).await;
    seed(&pool, &owner, 1, "-2 days", "1 day", Some("-3 days")).await;
    let before: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM mcp_credentials c WHERE user_id=$1",
    )
    .bind(Uuid::parse_str(owner.as_str()).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    let report = store.audit_mcp_credentials(&owner, None).await.unwrap();
    assert_eq!(report.counts.total, 110);
    assert_eq!(report.counts.active, 3);
    assert_eq!(report.counts.expired, 105);
    assert_eq!(report.counts.revoked, 2);
    assert_eq!(report.counts.issued_last_24h, 2);
    assert_eq!(report.counts.quota_used, 4);
    assert_eq!(report.remaining_issuance, 16);
    assert_eq!(report.counts.inconsistent, 1);
    assert!(!report.consistent);
    assert_eq!(report.items.len(), 100);
    let second = store
        .audit_mcp_credentials(&owner, report.next_cursor.as_deref())
        .await
        .unwrap();
    assert_eq!(second.counts.total, 110);
    assert_eq!(second.items.len(), 10);
    assert!(second.next_cursor.is_none());
    let mut ids = report
        .items
        .iter()
        .chain(&second.items)
        .map(|v| v.id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 110);
    assert!(
        report
            .items
            .iter()
            .chain(&second.items)
            .any(|v| v.issues.contains(&"revocation_before_creation".into()))
    );
    assert!(
        store
            .audit_mcp_credentials(&UserId::new(Uuid::new_v4().to_string()), None)
            .await
            .is_err()
    );
    let role = format!("mcp_ops_{}", Uuid::new_v4().simple());
    let password = Uuid::new_v4().simple().to_string();
    sqlx::raw_sql(&format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'; ALTER ROLE {role} SET default_transaction_read_only=on;
        GRANT SELECT (id) ON users TO {role};
        GRANT SELECT (id,user_id,scope,created_at,expires_at,revoked_at) ON mcp_credentials TO {role};")).execute(&pool).await.unwrap();
    let read_url = sqlx::postgres::PgConnectOptions::from_str(&url)
        .unwrap()
        .username(&role)
        .password(&password)
        .to_url_lossy()
        .to_string();
    let output = cli(&read_url, &owner);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stderr, [] as [u8; 0]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    for secret in [
        "private-host",
        "private-owner",
        "private-access-token",
        "token_digest",
        "host_name",
    ] {
        assert!(!stdout.contains(secret));
    }
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["counts"]["quota_used"], 4);
    let after: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM mcp_credentials c WHERE user_id=$1",
    )
    .bind(Uuid::parse_str(owner.as_str()).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    sqlx::raw_sql(&format!("DROP OWNED BY {role}; DROP ROLE {role}"))
        .execute(&pool)
        .await
        .unwrap();
    let (_, _, foreign_pool, foreign) = fixture().await;
    let isolated = store.audit_mcp_credentials(&foreign, None).await.unwrap();
    assert_eq!(isolated.counts.total, 0);
    assert!(isolated.items.is_empty());
    cleanup(&foreign_pool, &foreign).await;
    cleanup(&pool, &owner).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn mcp_audit_distinguishes_full_quota_from_overage_and_uses_one_snapshot() {
    let (url, store, pool, owner) = fixture().await;
    assert_eq!(cli(&url, &owner).status.code(), Some(0));
    seed(&pool, &owner, 20, "-2 days", "1 day", None).await;
    let full = store.audit_mcp_credentials(&owner, None).await.unwrap();
    assert!(full.consistent);
    assert_eq!(full.remaining_issuance, 0);
    seed(&pool, &owner, 1, "-2 days", "1 day", None).await;
    let over = store.audit_mcp_credentials(&owner, None).await.unwrap();
    assert_eq!(over.issues, vec!["issuance_quota_exceeded"]);
    assert_eq!(over.remaining_issuance, 0);
    assert_eq!(cli(&url, &owner).status.code(), Some(2));
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE mcp_credentials SET revoked_at=NOW() WHERE user_id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .execute(&mut *tx)
        .await
        .unwrap();
    let reads = async {
        for _ in 0..8 {
            let audit = store.audit_mcp_credentials(&owner, None).await.unwrap();
            assert_eq!(audit.counts.total, 21);
            assert_eq!(audit.counts.inconsistent, 0);
            assert!(audit.counts.active == 0 || audit.counts.active == 21);
            assert_eq!(
                audit.items.iter().filter(|v| v.status == "active").count(),
                usize::try_from(audit.counts.active).unwrap()
            );
            assert_eq!(audit.counts.quota_used, audit.counts.active);
        }
    };
    let ((), commit) = tokio::join!(reads, tx.commit());
    commit.unwrap();
    assert_eq!(cli(&url, &owner).status.code(), Some(0));
    let row = sqlx::query("SELECT count(*) AS n FROM mcp_credentials WHERE user_id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("n"), 21);
    seed(&pool, &owner, 1, "1 day", "2 days", None).await;
    seed(&pool, &owner, 1, "-2 days", "1 day", Some("1 hour")).await;
    let invalid = store.audit_mcp_credentials(&owner, None).await.unwrap();
    assert_eq!(invalid.counts.inconsistent, 2);
    assert!(
        invalid
            .items
            .iter()
            .any(|v| v.issues.contains(&"creation_in_future".into()))
    );
    assert!(
        invalid
            .items
            .iter()
            .any(|v| v.issues.contains(&"revocation_in_future".into()))
    );
    cleanup(&pool, &owner).await;
}
