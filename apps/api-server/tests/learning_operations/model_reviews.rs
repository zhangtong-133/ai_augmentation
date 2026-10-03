use super::*;

async fn seed(f: &Fixture, count: i32, today: bool) {
    sqlx::query("INSERT INTO learning_model_authorizations(user_id,request_id,plan_id,task_id,connection_id,connection_revision,model,input_digest,digest,status,created_ms,expires_ms) SELECT $1,gen_random_uuid(),gen_random_uuid(),gen_random_uuid(),gen_random_uuid(),9007199254740993,'private-model',repeat('a',64),repeat('b',64),'draft',CASE WHEN $3 THEN floor(extract(epoch FROM statement_timestamp())*1000)::bigint ELSE 0 END,CASE WHEN $3 THEN floor(extract(epoch FROM statement_timestamp())*1000)::bigint+300000 ELSE 300000 END FROM generate_series(1,$2)")
        .bind(f.id()).bind(count).bind(today).execute(&f.pool).await.unwrap();
    sqlx::query("UPDATE learning_model_authorizations SET status='cancelled' WHERE user_id=$1 AND status='draft'").bind(f.id()).execute(&f.pool).await.unwrap();
}
async fn snapshot(f: &Fixture) -> serde_json::Value {
    sqlx::query_scalar("SELECT jsonb_build_object('requests',(SELECT jsonb_agg(to_jsonb(a) ORDER BY request_id) FROM learning_model_authorizations a WHERE user_id=$1),'events',(SELECT jsonb_agg(to_jsonb(a) ORDER BY request_id,event) FROM learning_model_authorization_audit a WHERE user_id=$1))").bind(f.id()).fetch_one(&f.pool).await.unwrap()
}
#[tokio::test]
#[ignore = "需要有建角色权限的一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn model_metadata_reader_paginates_and_never_reads_private_columns_or_writes() {
    let _role_ddl = ROLE_DDL_LOCK.lock().await;
    let f = Fixture::new().await;
    seed(&f, 105, false).await;
    let before = snapshot(&f).await;
    let role = format!("learning_models_{}", Uuid::new_v4().simple());
    let password = Uuid::new_v4().simple().to_string();
    sqlx::raw_sql(&format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'; ALTER ROLE {role} SET default_transaction_read_only=on;
      GRANT USAGE ON SCHEMA public TO {role};
      GRANT SELECT(id) ON users TO {role};
      GRANT SELECT(user_id,request_id,plan_id,task_id,connection_id,connection_revision,status,created_ms,expires_ms,approved_ms,dispatch_deadline_ms,sent_ms) ON learning_model_authorizations TO {role};
      GRANT SELECT(user_id,request_id,event,at_ms) ON learning_model_authorization_audit TO {role};
      GRANT SELECT(user_id,request_id,status) ON learning_plans TO {role};
      GRANT SELECT(user_id,request_id,id,status) ON learning_tasks TO {role};
      GRANT SELECT(user_id,task_id,outcome) ON learning_results TO {role};
      GRANT SELECT(user_id,task_id,deleted) ON learning_evidence TO {role};
      GRANT SELECT(user_id,id,status,revision,expires_ms) ON subscription_connections TO {role};")).execute(&f.pool).await.unwrap();
    let limited = PgConnectOptions::from_str(&f.url)
        .unwrap()
        .username(&role)
        .password(&password)
        .to_url_lossy()
        .to_string();
    let out = cli(&limited, &["audit-models", "--user", f.owner.as_str()]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    for forbidden in [
        "private-model",
        "input_digest",
        "dispatch_token",
        "advice",
        "subject",
    ] {
        assert!(!text.contains(forbidden));
    }
    let first: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 100);
    assert_eq!(first["items"][0]["connection_revision"], "9007199254740993");
    let second = f
        .store
        .audit_learning_models(&f.owner, first["next_cursor"].as_str())
        .await
        .unwrap();
    assert!(second.consistent);
    assert_eq!(second.counts["cancelled"], 105);
    assert_eq!(second.items.len(), 5);
    assert!(second.next_cursor.is_none());
    let mut ids = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["request_id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    ids.extend(second.items.iter().map(|i| i.request_id.clone()));
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 105);
    let reader = sqlx::PgPool::connect(&limited).await.unwrap();
    for query in [
        "SELECT advice FROM learning_model_authorizations",
        "SELECT dispatch_token FROM learning_model_authorizations",
        "SELECT digest FROM learning_model_authorizations",
        "SELECT model FROM learning_model_authorizations",
        "SELECT body FROM learning_evidence",
        "SELECT task FROM learning_tasks",
        "SELECT note FROM learning_results",
        "SELECT subject_hash FROM subscription_connections",
        "SELECT version FROM _sqlx_migrations",
        "DELETE FROM learning_model_authorization_audit",
    ] {
        assert!(sqlx::query(query).execute(&reader).await.is_err());
    }
    reader.close().await;
    assert_eq!(snapshot(&f).await, before);
    sqlx::query(
        "DELETE FROM learning_model_authorization_audit WHERE user_id=$1 AND event='draft'",
    )
    .bind(f.id())
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        cli(&limited, &["audit-models", "--user", f.owner.as_str()])
            .status
            .code(),
        Some(2)
    );
    let empty = f
        .store
        .audit_learning_models(&f.owner, Some("ffffffff-ffff-ffff-ffff-ffffffffffff"))
        .await
        .unwrap();
    assert!(empty.items.is_empty());
    assert!(!empty.consistent);
    assert!(empty.issues.iter().any(|i| i == "missing_state_audit"));
    assert_eq!(empty.counts["inconsistent_authorizations"], 105);
    assert_eq!(
        cli(
            &limited,
            &["audit-models", "--user", &Uuid::new_v4().to_string()]
        )
        .status
        .code(),
        Some(1)
    );
    let other = Fixture::new().await;
    let report = f
        .store
        .audit_learning_models(&other.owner, None)
        .await
        .unwrap();
    assert!(report.consistent);
    assert_eq!(report.counts["authorizations"], 0);
    other.cleanup().await;
    sqlx::raw_sql(&format!("DROP OWNED BY {role}; DROP ROLE {role}"))
        .execute(&f.pool)
        .await
        .unwrap();
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_metadata_quotas_include_tombstones_and_distinguish_exhaustion_from_damage() {
    let f = Fixture::new().await;
    seed(&f, 980, false).await;
    seed(&f, 20, true).await;
    let full = f.store.audit_learning_models(&f.owner, None).await.unwrap();
    assert!(full.consistent, "{:?}", full.issues);
    assert_eq!(full.quotas["authorizations"].remaining, 0);
    assert_eq!(full.quotas["authorizations_today"].remaining, 0);
    assert_eq!(full.warnings.len(), 2);
    seed(&f, 1, true).await;
    let before = snapshot(&f).await;
    let over = f.store.audit_learning_models(&f.owner, None).await.unwrap();
    assert!(!over.consistent);
    assert!(
        over.issues
            .iter()
            .any(|i| i == "authorizations_limit_exceeded")
    );
    assert!(
        over.issues
            .iter()
            .any(|i| i == "authorizations_today_limit_exceeded")
    );
    assert_eq!(snapshot(&f).await, before);
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_metadata_reports_send_damage_and_unknown_without_repairing_or_retrying() {
    let f = Fixture::new().await;
    seed(&f, 1, false).await;
    sqlx::query("UPDATE learning_model_authorizations SET status='unknown',approved_ms=1,dispatch_deadline_ms=100,sent_ms=2 WHERE user_id=$1").bind(f.id()).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO learning_model_authorization_audit SELECT user_id,request_id,event,2 FROM learning_model_authorizations CROSS JOIN unnest(ARRAY['authorized','running','sending']) event WHERE user_id=$1").bind(f.id()).execute(&f.pool).await.unwrap();
    let before = snapshot(&f).await;
    let report = f.store.audit_learning_models(&f.owner, None).await.unwrap();
    assert!(report.consistent, "{:?}", report.issues);
    assert_eq!(report.warnings, ["outcome_unknown_no_retry"]);
    assert_eq!(snapshot(&f).await, before);
    sqlx::query("UPDATE learning_model_authorization_audit SET at_ms=3 WHERE user_id=$1 AND event='sending'").bind(f.id()).execute(&f.pool).await.unwrap();
    let report = f.store.audit_learning_models(&f.owner, None).await.unwrap();
    assert_eq!(report.issues, ["sending_audit_mismatch"]);
    f.cleanup().await;
}
