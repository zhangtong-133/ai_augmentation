use super::*;
use personal_ai_storage_postgres::PostgresStore;
const BASE: &str = "/api/knowledge/answer-authorizations";
async fn json_body(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 32768).await.unwrap()).unwrap()
}
#[tokio::test]
async fn answer_authorization_routes_check_session_and_csrf_before_store() {
    let (state, _, _, _, cookie, _) = retrieval_fixture().await;
    let app = router(state);
    let id = Uuid::new_v4();
    for (method, path) in [
        ("GET", BASE.into()),
        ("POST", BASE.into()),
        ("GET", format!("{BASE}/{id}")),
        ("POST", format!("{BASE}/{id}/approve")),
        ("POST", format!("{BASE}/{id}/cancel")),
    ] {
        assert_eq!(
            auth_request(app.clone(), method, &path, None, "{}", true)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        if method == "POST" {
            assert_eq!(
                auth_request(app.clone(), method, &path, Some(&cookie), "{}", false)
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
    }
    assert_eq!(
        auth_request(app, "GET", BASE, Some(&cookie), "", false)
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn answer_authorization_http_persists_exact_private_consent_without_inference() {
    use std::sync::atomic::Ordering;
    let (mut state, auth_store, embeddings, owner, cookie, document) = retrieval_fixture().await;
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let pg = Arc::new(PostgresStore::connect(&url).await.unwrap());
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    pg.save_user(&User {
        id: owner.clone(),
        email: format!("{}@auth-http.example", owner.as_str()),
        display_name: "Owner".into(),
    })
    .await
    .unwrap();
    pg.insert_document(&owner, "fixture", &document)
        .await
        .unwrap();
    state.answer_authorizations = Some(pg);
    let calls = embeddings.calls.load(Ordering::SeqCst);
    let app = router(state);
    let id = Uuid::new_v4().to_string();
    let path = format!("{BASE}/{id}");
    let input = json!({"request_id":id,"question":"私有问题","endpoint":"http://127.0.0.1:11435","model":"qwen3:4b-q4_K_M","sources":[{"document_id":document.summary.id,"ordinal":0}]});
    let response = auth_request(
        app.clone(),
        "POST",
        BASE,
        Some(&cookie),
        &input.to_string(),
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let saved = json_body(response).await;
    assert_eq!(saved["preview"]["materials"][0]["text"], document.chunks[0]);
    let mut forged = input.clone();
    forged["sources"][0]["text"] = json!("forged");
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            BASE,
            Some(&cookie),
            &forged.to_string(),
            true
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut changed = input.clone();
    changed["question"] = json!("changed");
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            BASE,
            Some(&cookie),
            &changed.to_string(),
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let mut remote = input.clone();
    remote["endpoint"] = json!("https://example.com");
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            BASE,
            Some(&cookie),
            &remote.to_string(),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let approve = json!({"digest":saved["digest"],"acknowledge_sharing":true,"acknowledge_local_compute":true});
    let response = auth_request(
        app.clone(),
        "POST",
        &format!("{path}/approve"),
        Some(&cookie),
        &approve.to_string(),
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["status"], "authorized");
    let listed =
        json_body(auth_request(app.clone(), "GET", BASE, Some(&cookie), "", false).await).await;
    assert_eq!(listed["execution_available"], false);
    assert!(listed["items"][0]["preview"].is_null());
    assert!(!listed.to_string().contains("私有问题"));
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            &format!("{path}/execute"),
            Some(&cookie),
            "{}",
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE documents SET title='changed' WHERE id=$1")
        .bind(Uuid::parse_str(&document.summary.id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let invalid =
        json_body(auth_request(app.clone(), "GET", &path, Some(&cookie), "", false).await).await;
    assert_eq!(invalid["status"], "invalidated");
    assert!(invalid["preview"].is_null());
    assert_eq!(embeddings.calls.load(Ordering::SeqCst), calls);
    // Pause the database operation after initial authentication, then revoke the session.
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    let mut pending_input = input.clone();
    pending_input["request_id"] = json!(Uuid::new_v4().to_string());
    let pending_app = app.clone();
    let pending_cookie = cookie.clone();
    let pending = tokio::spawn(async move {
        auth_request(
            pending_app,
            "POST",
            BASE,
            Some(&pending_cookie),
            &pending_input.to_string(),
            true,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let has_waiter: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))",
            )
            .bind(blocker)
            .fetch_one(&pool)
            .await
            .unwrap();
            if has_waiter {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    auth_store.sessions.lock().unwrap().clear();
    lock.commit().await.unwrap();
    let denied = pending.await.unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    assert!(json_body(denied).await.get("preview").is_none());
    assert_eq!(
        auth_request(app, "GET", &path, Some(&cookie), "", false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
}
