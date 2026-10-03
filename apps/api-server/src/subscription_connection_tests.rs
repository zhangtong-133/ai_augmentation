use super::*;
use personal_ai_storage::{
    MetadataStore,
    subscription_connections::{SubscriptionConnectionStore, VerifiedSubscriptionConnection},
};
use personal_ai_storage_postgres::PostgresStore;
struct Fixture {
    state: AppState,
    store: Arc<PostgresStore>,
    pool: sqlx::PgPool,
    owner: UserId,
    other: UserId,
    cookie: String,
    other_cookie: String,
}
impl Fixture {
    async fn new() -> Self {
        let (mut state, sessions, _, owner, cookie, _) = retrieval_fixture().await;
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let store = Arc::new(PostgresStore::connect(&url).await.unwrap());
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let mut user = sessions.get_user(&owner).await.unwrap();
        user.email = format!("{}@subscription-http.example", Uuid::new_v4());
        store.save_user(&user).await.unwrap();
        let other = User {
            id: UserId::new(Uuid::new_v4().to_string()),
            email: format!("{}@subscription-http.example", Uuid::new_v4()),
            display_name: "Other".into(),
        };
        sessions.save_user(&other).await.unwrap();
        store.save_user(&other).await.unwrap();
        let token = "e".repeat(64);
        sessions.sessions.lock().unwrap().insert(
            format!("{:x}", Sha256::digest(token.as_bytes())),
            other.id.to_string(),
        );
        state.subscription_connections = Some(store.clone());
        state.feed_values = Some(store.clone());
        Self {
            state,
            store,
            pool,
            owner,
            other: other.id,
            cookie,
            other_cookie: format!("personal_ai_session_v2={token}"),
        }
    }
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: serde_json::Value,
        cookie: Option<&str>,
        csrf: bool,
    ) -> axum::response::Response {
        auth_request(
            router(self.state.clone()),
            method,
            path,
            cookie,
            &body.to_string(),
            csrf,
        )
        .await
    }
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        decode(
            self.send(method, path, body, Some(&self.cookie), true)
                .await,
        )
        .await
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
async fn decode(response: axum::response::Response) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

impl Fixture {
    async fn connection(&self) -> String {
        let id = Uuid::new_v4().to_string();
        let time: i64 =
            sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
                .fetch_one(&self.pool)
                .await
                .unwrap();
        self.store
            .save_subscription_connection(
                &self.owner,
                &id,
                0,
                &VerifiedSubscriptionConnection {
                    host_id: Uuid::new_v4().urn().to_string(),
                    client_id: format!("oaiapp_{}", Uuid::new_v4().simple()),
                    subject: "private-subject".into(),
                    label: "personal".into(),
                    models: vec!["fixture".into()],
                    valid_until_unix_ms: time + 3_000_000,
                },
            )
            .await
            .unwrap();
        id
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn subscription_http_is_private_versioned_and_never_accepts_credentials() {
    let f = Fixture::new().await;
    let id = f.connection().await;
    let path = format!("/api/subscription-connections/{id}");
    let revoke = format!("{path}/revoke");
    for (method, url, body) in [
        ("GET", path.as_str(), json!({})),
        ("GET", "/api/subscription-connections", json!({})),
        ("POST", revoke.as_str(), json!({"revision":"1"})),
    ] {
        let response = f.send(method, url, body.clone(), None, true).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()["cache-control"], "no-store");
        if method == "POST" {
            let response = f.send(method, url, body, Some(&f.cookie), false).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(response.headers()["cache-control"], "no-store");
        }
    }
    let (status, detail) = f.call("GET", &path, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["revision"], "1");
    assert!(detail["valid_until_unix_ms"].is_string());
    assert_eq!(detail.as_object().unwrap().len(), 6);
    for private in [
        "subject",
        "client_id",
        "host_id",
        "access_token",
        "refresh_token",
        "email",
    ] {
        assert!(detail.get(private).is_none());
    }
    for (method, url, body) in [
        ("GET", path.as_str(), json!({})),
        ("POST", revoke.as_str(), json!({"revision":"1"})),
    ] {
        let response = f.send(method, url, body, Some(&f.other_cookie), true).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let (_, foreign) = decode(
        f.send(
            "GET",
            "/api/subscription-connections",
            json!({}),
            Some(&f.other_cookie),
            true,
        )
        .await,
    )
    .await;
    assert_eq!(foreign["items"], json!([]));
    for revision in [
        json!(1),
        json!("01"),
        json!("+1"),
        json!("0"),
        json!("1000"),
        json!("9223372036854775808"),
    ] {
        assert!(
            f.call("POST", &revoke, json!({"revision":revision}))
                .await
                .0
                .is_client_error()
        );
    }
    assert_eq!(
        f.call(
            "POST",
            &revoke,
            json!({"revision":"1","user_id":f.other.as_str()})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        f.call(
            "POST",
            &revoke,
            json!({"revision":"1","access_token":"fake"})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    for url in [
        "/api/subscription-connections?after=bad".into(),
        "/api/subscription-connections?user_id=other".into(),
        format!("{path}?unknown=1"),
        format!("{revoke}?user_id=other"),
    ] {
        let method = if url.contains("/revoke") {
            "POST"
        } else {
            "GET"
        };
        assert_eq!(
            f.call(method, &url, json!({"revision":"1"})).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        f.call(
            "POST",
            "/api/subscription-connections",
            json!({"access_token":"fake"})
        )
        .await
        .0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    let (status, revoked) = f.call("POST", &revoke, json!({"revision":"1"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revoked["status"], "revoked");
    assert_eq!(revoked["revision"], "2");
    assert_eq!(
        f.call("POST", &revoke, json!({"revision":"1"})).await.1,
        revoked
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn subscription_http_paginates_expires_and_rejects_stale_revocation() {
    let f = Fixture::new().await;
    let mut ids = Vec::new();
    for _ in 0..11 {
        ids.push(f.connection().await);
    }
    let (_, first) = f
        .call("GET", "/api/subscription-connections", json!({}))
        .await;
    assert_eq!(first["items"].as_array().unwrap().len(), 10);
    let after = first["next_cursor"].as_str().unwrap();
    let (_, second) = f
        .call(
            "GET",
            &format!("/api/subscription-connections?after={after}"),
            json!({}),
        )
        .await;
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert!(second["next_cursor"].is_null());
    assert!(after < second["items"][0]["id"].as_str().unwrap());
    let id = &ids[0];
    let path = format!("/api/subscription-connections/{id}");
    sqlx::query("UPDATE subscription_connections SET expires_ms=1 WHERE user_id=$1 AND id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .bind(Uuid::parse_str(id).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    let (_, expired) = f.call("GET", &path, json!({})).await;
    assert_eq!(expired["status"], "expired");
    assert_eq!(expired["revision"], "2");
    let response = f
        .send(
            "POST",
            &format!("{path}/revoke"),
            json!({"revision":"1"}),
            Some(&f.cookie),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        f.call("POST", &format!("{path}/revoke"), json!({"revision":"2"}))
            .await
            .1["status"],
        "revoked"
    );
    let response = f
        .send("POST", "/api/auth/logout", json!({}), Some(&f.cookie), true)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        f.call("GET", &path, json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    f.cleanup().await;
}
#[tokio::test]
async fn subscription_http_disabled_runtime_still_requires_session_and_csrf() {
    let (state, _, _, _, cookie, _) = retrieval_fixture().await;
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/subscription-connections")
                .header("authorization", format!("Bearer {}", state.api_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()["cache-control"], "no-store");

    let path = format!("/api/subscription-connections/{}/revoke", Uuid::new_v4());
    for (method, url, cookie, csrf, expected) in [
        ("GET", "/api/subscription-connections", None, true, 401),
        (
            "GET",
            "/api/subscription-connections",
            Some(cookie.as_str()),
            true,
            503,
        ),
        ("POST", path.as_str(), Some(cookie.as_str()), false, 403),
        ("POST", path.as_str(), Some(cookie.as_str()), true, 503),
    ] {
        let response = auth_request(
            router(state.clone()),
            method,
            url,
            cookie,
            "{\"revision\":\"1\"}",
            csrf,
        )
        .await;
        assert_eq!(response.status().as_u16(), expected);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
}

#[path = "feed_value_http_tests.rs"]
mod feed_value_http_tests;
