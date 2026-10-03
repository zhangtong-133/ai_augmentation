use super::*;
use personal_ai_feeds::transport::{FeedTransport, FetchError, FetchFuture};
use personal_ai_storage::{
    MetadataStore,
    feeds::{CollectionStatus, FeedStore},
};
use personal_ai_storage_postgres::PostgresStore;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct Transport {
    calls: AtomicUsize,
    mode: AtomicUsize,
}
impl FeedTransport for Transport {
    fn fetch(&self, _: &str) -> FetchFuture<'_, Result<Vec<u8>, FetchError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            if self.mode.load(Ordering::SeqCst) == 1 {
                return Err(FetchError::Timeout);
            }
            Ok(b"<rss version=\"2.0\"><channel><title>News</title><link>https://example.com/</link><description>Summary</description><item><guid>private-guid</guid><title>Title</title></item></channel></rss>".to_vec())
        })
    }
}
struct Fixture {
    state: AppState,
    store: Arc<PostgresStore>,
    pool: sqlx::PgPool,
    owner: UserId,
    other: UserId,
    cookie: String,
    other_cookie: String,
    transport: Arc<Transport>,
}
impl Fixture {
    async fn new(enabled: bool) -> Self {
        let (mut state, sessions, _, owner, cookie, _) = retrieval_fixture().await;
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let store = Arc::new(PostgresStore::connect(&url).await.unwrap());
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let mut user = sessions.get_user(&owner).await.unwrap();
        user.email = format!("{}@rss-http.example", Uuid::new_v4());
        store.save_user(&user).await.unwrap();
        let other = User {
            id: UserId::new(Uuid::new_v4().to_string()),
            email: format!("{}@rss-http.example", Uuid::new_v4()),
            display_name: "Other".into(),
        };
        sessions.save_user(&other).await.unwrap();
        store.save_user(&other).await.unwrap();
        let token = "e".repeat(64);
        sessions.sessions.lock().unwrap().insert(
            format!("{:x}", Sha256::digest(token.as_bytes())),
            other.id.to_string(),
        );
        let transport = Arc::new(Transport::default());
        state.feeds = Some(Arc::new(FeedRuntime::new(
            store.clone(),
            enabled.then(|| transport.clone() as Arc<dyn FeedTransport>),
        )));
        Self {
            state,
            store,
            pool,
            owner,
            other: other.id,
            cookie,
            other_cookie: format!("personal_ai_session_v2={token}"),
            transport,
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
    async fn sub(&self) -> serde_json::Value {
        let (status, value) = self.call("POST", "/api/feed-subscriptions", input()).await;
        assert_eq!(status, StatusCode::CREATED);
        value
    }
    async fn preview(&self, sub: &serde_json::Value) -> serde_json::Value {
        let path = format!(
            "/api/feed-subscriptions/{}/collections",
            sub["snapshot"]["subscription_id"].as_str().unwrap()
        );
        let (status, value) = self
            .call("POST", &path, json!({"request_id":Uuid::new_v4()}))
            .await;
        assert_eq!(status, StatusCode::CREATED);
        value
    }
    async fn assert_private_pages(&self, sub: &serde_json::Value, draft: &serde_json::Value) {
        for (route, expected) in [
            ("/api/feed-subscriptions", sub),
            ("/api/feed-collections", draft),
        ] {
            let (status, page) = self.call("GET", route, json!({})).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(page["items"], json!([expected]));
            assert!(page["next_cursor"].is_null());
            let (status, page) = decode(
                self.send("GET", route, json!({}), Some(&self.other_cookie), false)
                    .await,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(page["items"], json!([]));
        }
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
fn input() -> serde_json::Value {
    json!({"id":Uuid::new_v4(),"name":"私有订阅","source_url":"https://EXAMPLE.com:443/rss?secret=private#fragment","enabled":true})
}
fn confirm(draft: &serde_json::Value) -> serde_json::Value {
    json!({"accepted_digest":draft["digest"],"acknowledge_source_request":true})
}
fn path(draft: &serde_json::Value) -> String {
    format!(
        "/api/feed-collections/{}",
        draft["plan"]["request_id"].as_str().unwrap()
    )
}

#[tokio::test]
async fn feed_http_requires_session_csrf_strict_payloads_and_available_runtime() {
    let (state, _, _, _, cookie, _) = retrieval_fixture().await;
    let app = router(state);
    for route in [
        "/api/feeds/config",
        "/api/feed-subscriptions",
        "/api/feed-collections",
    ] {
        assert_eq!(
            auth_request(app.clone(), "GET", route, None, "", false)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            auth_request(app.clone(), "GET", route, Some(&cookie), "", false)
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/feed-subscriptions",
            Some(&cookie),
            &input().to_string(),
            false
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let mut forged = input();
    forged["user_id"] = json!(Uuid::new_v4());
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/feed-subscriptions",
            Some(&cookie),
            &forged.to_string(),
            true
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/feed-subscriptions")
                .header(
                    "authorization",
                    "Bearer test-admin-token-01234567890123456789",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()["cache-control"], "no-store");
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn feed_http_disabled_mode_manages_private_previews_without_claiming_or_fetching() {
    let f = Fixture::new(false).await;
    assert_eq!(
        f.call("GET", "/api/feeds/config", json!({})).await.1,
        json!({"mode":"disabled","execution_enabled":false})
    );
    let sub = f.sub().await;
    let key = sub["snapshot"]["subscription_id"].as_str().unwrap();
    let subpath = format!("/api/feed-subscriptions/{key}");
    assert_eq!(sub["snapshot"]["revision"], "1");
    assert_eq!(
        sub["snapshot"]["source_url"],
        "https://example.com/rss?secret=private"
    );
    let draft = f.preview(&sub).await;
    let path = path(&draft);
    assert!(draft["plan"]["created_at_unix_ms"].is_string());
    assert!(draft["plan"]["subscription_revision"].is_string());
    let result = f
        .call("POST", &format!("{path}/confirm"), confirm(&draft))
        .await;
    assert_eq!(result.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(result.1["error"]["code"], "feed_execution_disabled");
    assert_eq!(
        f.store
            .get_collection(&f.owner, draft["plan"]["request_id"].as_str().unwrap())
            .await
            .unwrap()
            .status,
        CollectionStatus::Draft
    );
    for route in [
        &subpath,
        &format!("{subpath}/entries"),
        &path,
        &format!("{path}/audit"),
    ] {
        let response = f
            .send("GET", route, json!({}), Some(&f.other_cookie), true)
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    f.assert_private_pages(&sub, &draft).await;
    for method in ["PUT", "DELETE"] {
        let response = f
            .send(
                method,
                &subpath,
                json!({"revision":"1"}),
                Some(&f.cookie),
                false,
            )
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let update = json!({"revision":"1","name":"修改","source_url":"https://example.com/new","enabled":false});
    for bad in [
        json!(1),
        json!("01"),
        json!("0"),
        json!("18446744073709551615"),
    ] {
        let mut body = update.clone();
        body["revision"] = bad;
        assert!(f.call("PUT", &subpath, body).await.0.is_client_error());
    }
    assert_eq!(
        f.call("PUT", &subpath, update.clone()).await.1["snapshot"]["revision"],
        "2"
    );
    assert_eq!(
        f.call("PUT", &subpath, update).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.call("POST", &format!("{path}/cancel"), json!({})).await.1["status"],
        "cancelled"
    );
    assert_eq!(
        f.call("DELETE", &subpath, json!({"revision":"2"})).await.0,
        StatusCode::OK
    );
    assert_eq!(
        f.call("GET", &subpath, json!({})).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(f.transport.calls.load(Ordering::SeqCst), 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn feed_http_confirms_once_and_never_exposes_execution_credentials() {
    let f = Fixture::new(true).await;
    let sub = f.sub().await;
    let draft = f.preview(&sub).await;
    let path = path(&draft);
    for suffix in ["confirm", "cancel", "recover"] {
        assert_eq!(
            f.send(
                "POST",
                &format!("{path}/{suffix}"),
                confirm(&draft),
                Some(&f.cookie),
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        f.send(
            "POST",
            &format!("{path}/confirm"),
            confirm(&draft),
            Some(&f.other_cookie),
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    for extra in [
        json!({"claim_id":Uuid::new_v4()}),
        json!({"source_url":"https://evil.example/"}),
        json!({"acknowledge_source_request":false}),
        json!({"accepted_digest":"0".repeat(64)}),
    ] {
        let mut body = confirm(&draft);
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(
            f.call("POST", &format!("{path}/confirm"), body)
                .await
                .0
                .is_client_error()
        );
    }
    assert_eq!(f.transport.calls.load(Ordering::SeqCst), 0);
    let (status, result) = f
        .call("POST", &format!("{path}/confirm"), confirm(&draft))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["counts"]["inserted"], 1);
    assert!(result["claimed_at_unix_ms"].is_string());
    assert!(result.get("claim_id").is_none());
    assert_eq!(
        f.call("POST", &format!("{path}/confirm"), confirm(&draft))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(f.call("GET", &path, json!({})).await.1, result);
    let audit = f.call("GET", &format!("{path}/audit"), json!({})).await.1;
    assert_eq!(audit.as_array().unwrap().len(), 3);
    assert!(!audit.to_string().contains("secret"));
    let entries = format!(
        "/api/feed-subscriptions/{}/entries",
        sub["snapshot"]["subscription_id"].as_str().unwrap()
    );
    assert_eq!(
        f.call("GET", &entries, json!({})).await.1["items"][0]["title"],
        "Title"
    );
    for suffix in ["claim", "finish"] {
        assert_eq!(
            f.call("POST", &format!("{path}/{suffix}"), json!({}))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    for route in [
        "/api/feed-subscriptions?after=invalid",
        "/api/feed-collections?user_id=forged",
        &format!("{entries}?after=invalid"),
    ] {
        assert_eq!(
            f.call("GET", route, json!({})).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(f.transport.calls.load(Ordering::SeqCst), 1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn feed_http_unknown_results_are_queried_without_retry_and_recovery_is_explicit() {
    let f = Fixture::new(true).await;
    let sub = f.sub().await;
    let draft = f.preview(&sub).await;
    let path = path(&draft);
    f.transport.mode.store(1, Ordering::SeqCst);
    assert_eq!(
        f.call("POST", &format!("{path}/confirm"), confirm(&draft))
            .await
            .1["status"],
        "unknown"
    );
    assert_eq!(f.call("GET", &path, json!({})).await.1["status"], "unknown");
    assert_eq!(
        f.call("POST", &format!("{path}/confirm"), confirm(&draft))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let saved = f.preview(&sub).await;
    let request = saved["plan"]["request_id"].as_str().unwrap();
    let _claim = f
        .store
        .claim_collection(&f.owner, request, saved["digest"].as_str().unwrap())
        .await
        .unwrap();
    sqlx::query("UPDATE feed_collections SET claimed_ms=claimed_ms-60001,deadline_ms=deadline_ms-60001 WHERE user_id=$1 AND request_id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(request).unwrap()).execute(&f.pool).await.unwrap();
    let path = format!("/api/feed-collections/{request}");
    assert_eq!(f.call("GET", &path, json!({})).await.1["status"], "running");
    assert_eq!(
        f.call("POST", &format!("{path}/recover"), json!({}))
            .await
            .1["status"],
        "unknown"
    );
    assert_eq!(f.transport.calls.load(Ordering::SeqCst), 1);
    f.cleanup().await;
}

#[path = "feed_tests/briefs.rs"]
mod briefs;

#[path = "feed_tests/schedules.rs"]
mod schedules;
