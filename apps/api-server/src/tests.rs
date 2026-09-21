use super::*;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use personal_ai_storage::documents::{DocumentStore, DocumentSummary, StoredDocument};
use personal_ai_storage::{BoxFuture, StorageResult};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Mutex};
use tower::ServiceExt;

const TOKEN: &str = "test-only-token-01234567890123456789";

#[derive(Default)]
struct MemoryStore {
    documents: Mutex<HashMap<String, (String, String, StoredDocument)>>,
    passwords: Mutex<HashMap<String, String>>,
    sessions: Mutex<HashMap<String, String>>,
    users: Mutex<HashMap<String, User>>,
    unavailable: bool,
}

impl MetadataStore for MemoryStore {
    fn password_hash(&self, email: &str) -> BoxFuture<'_, StorageResult<(UserId, String)>> {
        let result = self
            .users
            .lock()
            .unwrap()
            .values()
            .find(|u| u.email == email)
            .and_then(|u| {
                self.passwords
                    .lock()
                    .unwrap()
                    .get(u.id.as_str())
                    .map(|hash| (u.id.clone(), hash.clone()))
            })
            .ok_or(StorageError::NotFound);
        Box::pin(async { result })
    }
    fn set_password(&self, id: &UserId, hash: &str) -> BoxFuture<'_, StorageResult<()>> {
        let exists = self.users.lock().unwrap().contains_key(id.as_str());
        if exists {
            self.passwords
                .lock()
                .unwrap()
                .insert(id.to_string(), hash.into());
            self.sessions
                .lock()
                .unwrap()
                .retain(|_, value| value != id.as_str());
        }
        Box::pin(async move {
            if exists {
                Ok(())
            } else {
                Err(StorageError::NotFound)
            }
        })
    }
    fn create_session(
        &self,
        id: &UserId,
        digest: &str,
        hash: &str,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let valid = self
            .passwords
            .lock()
            .unwrap()
            .get(id.as_str())
            .is_some_and(|value| value == hash);
        if valid {
            self.sessions
                .lock()
                .unwrap()
                .insert(digest.into(), id.to_string());
        }
        Box::pin(async move {
            if valid {
                Ok(())
            } else {
                Err(StorageError::NotFound)
            }
        })
    }
    fn session_user(&self, digest: &str) -> BoxFuture<'_, StorageResult<User>> {
        let result = self
            .sessions
            .lock()
            .unwrap()
            .get(digest)
            .and_then(|id| self.users.lock().unwrap().get(id).cloned())
            .ok_or(StorageError::NotFound);
        Box::pin(async { result })
    }
    fn delete_session(&self, digest: &str) -> BoxFuture<'_, StorageResult<()>> {
        self.sessions.lock().unwrap().remove(digest);
        Box::pin(async { Ok(()) })
    }
    fn health(&self) -> BoxFuture<'_, StorageResult<()>> {
        Box::pin(async {
            if self.unavailable {
                Err(StorageError::Unavailable(
                    "secret connection details".into(),
                ))
            } else {
                Ok(())
            }
        })
    }
    fn get_user(&self, id: &UserId) -> BoxFuture<'_, StorageResult<User>> {
        let result = self
            .users
            .lock()
            .unwrap()
            .get(id.as_str())
            .cloned()
            .ok_or(StorageError::NotFound);
        Box::pin(async { result })
    }
    fn save_user(&self, user: &User) -> BoxFuture<'_, StorageResult<()>> {
        let mut users = self.users.lock().unwrap();
        let result = if users
            .values()
            .any(|u| u.email.eq_ignore_ascii_case(&user.email))
        {
            Err(StorageError::Conflict("duplicate".into()))
        } else {
            users.insert(user.id.to_string(), user.clone());
            Ok(())
        };
        Box::pin(async { result })
    }
}

fn app() -> Router {
    let store = Arc::new(MemoryStore::default());
    router(AppState {
        answering: None,
        indexing: None,
        web_importer: Arc::new(personal_ai_web_import::PublicWebImporter::default()),
        documents: store.clone(),
        store,
        api_token: Arc::from(TOKEN),
        auth: Arc::new(AuthConfig::new(false)),
    })
}

async fn request(
    app: Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn create_read_and_duplicate_user() {
    let app = app();
    let body = r#"{"email":" Alice@Example.com ","display_name":" Alice "}"#;
    let (status, user) = request(app.clone(), "POST", "/api/users", Some(TOKEN), body).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(user["email"], "alice@example.com");
    assert_eq!(user["display_name"], "Alice");
    let path = format!("/api/users/{}", user["id"].as_str().unwrap());
    let (status, fetched) = request(app.clone(), "GET", &path, Some(TOKEN), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched, user);
    assert_eq!(
        request(app, "POST", "/api/users", Some(TOKEN), body)
            .await
            .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn rejects_unauthorized_and_invalid_requests() {
    let app = app();
    for token in [None, Some("wrong-token")] {
        assert_eq!(
            request(app.clone(), "POST", "/api/users", token, "{}")
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    for body in [
        r#"{"email":"bad","display_name":"A"}"#,
        r#"{"email":"a@b","display_name":"  "}"#,
    ] {
        assert_eq!(
            request(app.clone(), "POST", "/api/users", Some(TOKEN), body)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        request(app.clone(), "POST", "/api/users", Some(TOKEN), "{")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            app.clone(),
            "POST",
            "/api/users",
            Some(TOKEN),
            &"x".repeat(17000)
        )
        .await
        .0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        request(app.clone(), "GET", "/api/users/no-uuid", Some(TOKEN), "")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let missing = format!("/api/users/{}", Uuid::new_v4());
    assert_eq!(
        request(app, "GET", &missing, Some(TOKEN), "").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn readiness_checks_storage_but_liveness_does_not() {
    let app = router(AppState {
        answering: None,
        indexing: None,
        web_importer: Arc::new(personal_ai_web_import::PublicWebImporter::default()),
        documents: Arc::new(MemoryStore::default()),
        store: Arc::new(MemoryStore {
            unavailable: true,
            ..MemoryStore::default()
        }),
        api_token: Arc::from(TOKEN),
        auth: Arc::new(AuthConfig::new(false)),
    });
    assert_eq!(
        request(app.clone(), "GET", "/healthz", None, "").await.0,
        StatusCode::OK
    );
    let (status, body) = request(app, "GET", "/readyz", None, "").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body, json!({"error": {"code": "storage_unavailable"}}));
}

#[test]
fn configuration_rejects_missing_or_weak_credentials() {
    assert!(Config::load(|_| None).is_err());
    let values = |key: &str| match key {
        "DATABASE_URL" => Some("postgres://localhost/test".into()),
        "API_AUTH_TOKEN" => Some(TOKEN.into()),
        _ => None,
    };
    assert_eq!(
        Config::load(values).unwrap().address.to_string(),
        "127.0.0.1:8080"
    );
    assert!(
        Config::load(|key| if key == "API_AUTH_TOKEN" {
            Some("short".into())
        } else {
            values(key)
        })
        .is_err()
    );
}
async fn auth_request(
    app: Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: &str,
    csrf: bool,
) -> axum::response::Response {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(cookie) = cookie {
        req = req.header("cookie", cookie);
    }
    if csrf {
        req = req.header("x-requested-with", "personal-ai");
    }
    app.oneshot(req.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // 在同一测试中覆盖完整的端到端会话生命周期。
async fn login_cookie_logout_and_password_reset() {
    let app = app();
    let (_, user) = request(
        app.clone(),
        "POST",
        "/api/users",
        Some(TOKEN),
        r#"{"email":"me@example.com","display_name":"我"}"#,
    )
    .await;
    let password_path = format!("/api/users/{}/password", user["id"].as_str().unwrap());
    assert_eq!(
        request(app.clone(), "POST", &password_path, None, "{}")
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            app.clone(),
            "POST",
            &password_path,
            Some(TOKEN),
            r#"{"password":"correct-password-123"}"#
        )
        .await
        .0,
        StatusCode::OK
    );
    let credentials = r#"{"email":"me@example.com","password":"correct-password-123"}"#;
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/auth/login",
            None,
            credentials,
            false
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/auth/login",
            None,
            r#"{"email":"me@example.com","password":"wrong"}"#,
            true
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let logged_in = auth_request(
        app.clone(),
        "POST",
        "/api/auth/login",
        None,
        credentials,
        true,
    )
    .await;
    assert_eq!(logged_in.status(), StatusCode::OK);
    let set_cookie = logged_in.headers()["set-cookie"].to_str().unwrap();
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Strict"));
    let cookie = set_cookie.split(';').next().unwrap().to_string();
    let identity = auth_request(app.clone(), "GET", "/api/auth/me", Some(&cookie), "", false).await;
    assert_eq!(identity.status(), StatusCode::OK);
    assert_eq!(identity.headers()["cache-control"], "no-store");
    let identity: serde_json::Value =
        serde_json::from_slice(&to_bytes(identity.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(identity, user);
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/auth/logout",
            Some(&cookie),
            "",
            false
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/auth/logout",
            Some(&cookie),
            "",
            true
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        auth_request(app.clone(), "GET", "/api/auth/me", Some(&cookie), "", false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let logged_in = auth_request(
        app.clone(),
        "POST",
        "/api/auth/login",
        None,
        credentials,
        true,
    )
    .await;
    let cookie = logged_in.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(
        request(
            app.clone(),
            "POST",
            &password_path,
            Some(TOKEN),
            r#"{"password":"new-password-456789"}"#
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        auth_request(app.clone(), "GET", "/api/auth/me", Some(&cookie), "", false)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        auth_request(app, "POST", "/api/auth/login", None, credentials, true)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn login_attempt_budget_is_enforced() {
    let app = app();
    // 格式错误的请求也会消耗尝试次数，但不执行高开销的密码运算。
    for _ in 0..20 {
        assert_eq!(
            auth_request(app.clone(), "POST", "/api/auth/login", None, "{", true)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        auth_request(app, "POST", "/api/auth/login", None, "{", true)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}
impl DocumentStore for MemoryStore {
    fn document_stats(
        &self,
        owner: &UserId,
        start_ms: i64,
        end_ms: i64,
    ) -> BoxFuture<'_, StorageResult<personal_ai_storage::documents::DocumentStats>> {
        let mut stats = personal_ai_storage::documents::DocumentStats::default();
        for (user, _, document) in self.documents.lock().unwrap().values() {
            if user != owner.as_str() {
                continue;
            }
            stats.total_documents += 1;
            stats.total_chunks += i64::from(document.summary.chunk_count);
            if (start_ms..end_ms).contains(&document.summary.created_at_unix_ms) {
                stats.imported_today += 1;
            }
        }
        let unavailable = self.unavailable;
        Box::pin(async move {
            if unavailable {
                Err(StorageError::Unavailable(
                    "secret connection details".into(),
                ))
            } else {
                Ok(stats)
            }
        })
    }
    fn insert_document(
        &self,
        owner: &UserId,
        digest: &str,
        document: &StoredDocument,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let mut documents = self.documents.lock().unwrap();
        let result = if documents
            .values()
            .any(|(user, hash, _)| user == owner.as_str() && hash == digest)
        {
            Err(StorageError::Conflict("duplicate".into()))
        } else {
            documents.insert(
                document.summary.id.clone(),
                (owner.to_string(), digest.into(), document.clone()),
            );
            Ok(())
        };
        Box::pin(async { result })
    }
    fn list_documents(
        &self,
        owner: &UserId,
        offset: u32,
    ) -> BoxFuture<'_, StorageResult<Vec<DocumentSummary>>> {
        let mut documents: Vec<_> = self
            .documents
            .lock()
            .unwrap()
            .values()
            .filter(|(user, _, _)| user == owner.as_str())
            .map(|(_, _, d)| d.summary.clone())
            .collect();
        documents.sort_by(|a, b| (b.created_at_unix_ms, &b.id).cmp(&(a.created_at_unix_ms, &a.id)));
        let documents = documents
            .into_iter()
            .skip(offset as usize)
            .take(20)
            .collect();
        Box::pin(async { Ok(documents) })
    }
    fn get_document(
        &self,
        owner: &UserId,
        id: &str,
    ) -> BoxFuture<'_, StorageResult<StoredDocument>> {
        let result = self
            .documents
            .lock()
            .unwrap()
            .get(id)
            .filter(|(user, _, _)| user == owner.as_str())
            .map(|(_, _, doc)| doc.clone())
            .ok_or(StorageError::NotFound);
        Box::pin(async { result })
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn documents_are_private_deduplicated_and_validated() {
    let store = Arc::new(MemoryStore::default());
    let owner = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: "owner@example.com".into(),
        display_name: "Owner".into(),
    };
    let other = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: "other@example.com".into(),
        display_name: "Other".into(),
    };
    store.save_user(&owner).await.unwrap();
    store.save_user(&other).await.unwrap();
    let token = "a".repeat(64);
    let second = "b".repeat(64);
    store.sessions.lock().unwrap().insert(
        format!("{:x}", Sha256::digest(token.as_bytes())),
        owner.id.to_string(),
    );
    store.sessions.lock().unwrap().insert(
        format!("{:x}", Sha256::digest(second.as_bytes())),
        other.id.to_string(),
    );
    let app = router(AppState {
        answering: None,
        indexing: None,
        web_importer: Arc::new(personal_ai_web_import::PublicWebImporter::default()),
        documents: store.clone(),
        store,
        api_token: Arc::from(TOKEN),
        auth: Arc::new(AuthConfig::new(false)),
    });
    let cookie = format!("personal_ai_session_v2={token}");
    let other_cookie = format!("personal_ai_session_v2={second}");
    assert_eq!(
        request(app.clone(), "GET", "/api/overview", Some(TOKEN), "")
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let empty = read_overview(app.clone(), &cookie).await;
    assert_eq!(
        empty["knowledge"],
        json!({"total_documents":0,"total_chunks":0,"imported_today":0})
    );
    let content = "# 标题\n\n".to_owned() + &"个人知识".repeat(3000);
    let body = json!({"title":"笔记","markdown":content,"source":"note.md","tags":["Rust","Rust"]})
        .to_string();
    assert_eq!(
        auth_request(app.clone(), "POST", "/api/documents", None, &body, true)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/documents",
            Some(&cookie),
            &body,
            false
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let response = auth_request(
        app.clone(),
        "POST",
        "/api/documents",
        Some(&cookie),
        &body,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let summary: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert!(summary["chunk_count"].as_i64().unwrap() > 1);
    assert_eq!(summary["tags"], json!(["Rust"]));
    let overview = read_overview(app.clone(), &cookie).await;
    assert_eq!(overview["timezone"], "UTC");
    assert_eq!(overview["knowledge"]["total_documents"], 1);
    assert_eq!(
        overview["knowledge"]["total_chunks"],
        summary["chunk_count"]
    );
    assert_eq!(overview["knowledge"]["imported_today"], 1);
    assert_eq!(
        read_overview(app.clone(), &other_cookie).await["knowledge"]["total_documents"],
        0
    );
    let path = format!("/api/documents/{}", summary["id"].as_str().unwrap());
    assert_eq!(
        auth_request(app.clone(), "GET", &path, Some(&other_cookie), "", false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let response = auth_request(app.clone(), "GET", &path, Some(&cookie), "", false).await;
    let doc: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 200_000).await.unwrap()).unwrap();
    assert_eq!(doc["markdown"], content);
    assert!(
        doc["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c.as_str().unwrap().chars().count() <= 1000)
    );
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/documents",
            Some(&cookie),
            &body,
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/documents",
            Some(&other_cookie),
            &body,
            true
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    for bad in [
        json!({"title":"x"}),
        json!({"title":"x","pdf_base64":"%%%"}),
        json!({"title":"x","pdf_base64":"bm90IGEgcGRm"}),
        json!({"title":"x","markdown":"text","pdf_base64":"JVBERi0="}),
        json!({"title":"","markdown":"text"}),
        json!({"title":"x","markdown":"\u{0000}"}),
        json!({"title":"x","markdown":" "}),
    ] {
        assert_eq!(
            auth_request(
                app.clone(),
                "POST",
                "/api/documents",
                Some(&cookie),
                &bad.to_string(),
                true
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let oversized = json!({"title":"x","markdown":"x".repeat(262_145)}).to_string();
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/documents",
            Some(&cookie),
            &oversized,
            true
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        auth_request(
            app.clone(),
            "GET",
            "/api/documents?offset=bad",
            Some(&cookie),
            "",
            false
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let response = auth_request(
        app,
        "GET",
        "/api/documents?offset=20",
        Some(&cookie),
        "",
        false,
    )
    .await;
    assert_eq!(to_bytes(response.into_body(), 1024).await.unwrap(), "[]");
}

async fn read_overview(app: Router, cookie: &str) -> serde_json::Value {
    let response = auth_request(app, "GET", "/api/overview", Some(cookie), "", false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn overview_storage_failure_is_not_an_empty_library() {
    let store = Arc::new(MemoryStore::default());
    let user = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: "stats@example.com".into(),
        display_name: "Stats".into(),
    };
    store.save_user(&user).await.unwrap();
    let token = "c".repeat(64);
    store.sessions.lock().unwrap().insert(
        format!("{:x}", Sha256::digest(token.as_bytes())),
        user.id.to_string(),
    );
    let app = router(AppState {
        answering: None,
        indexing: None,
        web_importer: Arc::new(personal_ai_web_import::PublicWebImporter::default()),
        documents: Arc::new(MemoryStore {
            unavailable: true,
            ..MemoryStore::default()
        }),
        store,
        api_token: Arc::from(TOKEN),
        auth: Arc::new(AuthConfig::new(false)),
    });
    let response = auth_request(
        app,
        "GET",
        "/api/overview",
        Some(&format!("personal_ai_session_v2={token}")),
        "",
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024).await.unwrap()).unwrap();
    assert_eq!(body, json!({"error":{"code":"storage_unavailable"}}));
}

#[derive(Default)]
struct FixtureWebImporter(std::sync::atomic::AtomicUsize);
impl personal_ai_knowledge::web::WebImporter for FixtureWebImporter {
    fn import(
        &self,
        url: &str,
    ) -> BoxFuture<
        '_,
        Result<personal_ai_knowledge::web::WebPage, personal_ai_knowledge::web::WebImportError>,
    > {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let result = match url {
            "https://public.example/article" => Ok(personal_ai_knowledge::web::WebPage {
                title: "网页标题".into(),
                source: "https://public.example/final".into(),
                text: "中文正文 **literal**".into(),
                html: "<article>中文正文 **literal**</article>".into(),
            }),
            _ => Err(personal_ai_knowledge::web::WebImportError::Timeout),
        };
        Box::pin(async { result })
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // 在同一测试中覆盖认证后的完整导入与隔离流程。
async fn web_import_requires_auth_and_csrf_then_persists_private_content() {
    let store = Arc::new(MemoryStore::default());
    let mut cookies = Vec::new();
    for token in ["a".repeat(64), "b".repeat(64)] {
        let user = User {
            id: UserId::new(Uuid::new_v4().to_string()),
            email: format!("{}@example.com", Uuid::new_v4()),
            display_name: "Web Owner".into(),
        };
        store.save_user(&user).await.unwrap();
        store.sessions.lock().unwrap().insert(
            format!("{:x}", Sha256::digest(token.as_bytes())),
            user.id.to_string(),
        );
        cookies.push(format!("personal_ai_session_v2={token}"));
    }
    let importer = Arc::new(FixtureWebImporter::default());
    let app = router(AppState {
        answering: None,
        indexing: None,
        web_importer: importer.clone(),
        documents: store.clone(),
        store,
        api_token: Arc::from(TOKEN),
        auth: Arc::new(AuthConfig::new(false)),
    });
    let body = json!({"url":"https://public.example/article","tags":["学习"]}).to_string();
    assert_eq!(
        auth_request(app.clone(), "POST", "/api/documents", None, &body, true)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/documents",
            Some(&cookies[0]),
            &body,
            false
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    for bad in [
        json!({"url":"https://public.example/article","markdown":"mixed"}),
        json!({"url":"https://public.example/article","pdf_base64":"JVBERg=="}),
        json!({"url":"https://public.example/article","source":"fake-source"}),
    ] {
        assert_eq!(
            auth_request(
                app.clone(),
                "POST",
                "/api/documents",
                Some(&cookies[0]),
                &bad.to_string(),
                true
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(importer.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    let response = auth_request(
        app.clone(),
        "POST",
        "/api/documents",
        Some(&cookies[0]),
        &body,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let summary: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(summary["title"], "网页标题");
    assert_eq!(summary["source_type"], "web_page");
    assert_eq!(summary["source"], "https://public.example/final");
    let path = format!("/api/documents/{}", summary["id"].as_str().unwrap());
    let response = auth_request(app.clone(), "GET", &path, Some(&cookies[0]), "", true).await;
    let document: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(document["markdown"], "中文正文 **literal**");
    assert_eq!(document["chunks"], json!(["中文正文 **literal**"]));
    assert!(document.get("original_html").is_none());
    assert_eq!(
        auth_request(app.clone(), "GET", &path, Some(&cookies[1]), "", true)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/documents",
            Some(&cookies[0]),
            &body,
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/documents",
            Some(&cookies[1]),
            &body,
            true
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let failing = json!({"url":"https://public.example/timeout"}).to_string();
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/documents",
            Some(&cookies[0]),
            &failing,
            true
        )
        .await
        .status(),
        StatusCode::GATEWAY_TIMEOUT
    );
    assert_eq!(
        read_overview(app, &cookies[0]).await["knowledge"]["total_documents"],
        1
    );
}

#[derive(Default)]
struct IndexDependencies {
    calls: std::sync::atomic::AtomicUsize,
    mode: std::sync::atomic::AtomicUsize,
    points: Mutex<HashMap<(String, String), personal_ai_storage::EmbeddingRecord>>,
}
impl personal_ai_llm::EmbeddingProvider for IndexDependencies {
    fn embedding(
        &self,
        input: &[String],
    ) -> personal_ai_llm::BoxFuture<'_, personal_ai_llm::LlmResult<Vec<personal_ai_llm::Embedding>>>
    {
        use std::sync::atomic::Ordering;
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mode = self.mode.load(Ordering::SeqCst);
        let len = input.len();
        Box::pin(async move {
            if mode == 1 {
                return Err(personal_ai_llm::LlmError::RateLimited);
            }
            Ok((0..if mode == 2 { len - 1 } else { len })
                .map(|_| personal_ai_llm::Embedding {
                    values: vec![1.0, 0.0],
                    model: "m".into(),
                })
                .collect())
        })
    }
}
impl personal_ai_storage::VectorStore for IndexDependencies {
    fn insert_embeddings(
        &self,
        owner: &UserId,
        records: &[personal_ai_storage::EmbeddingRecord],
    ) -> BoxFuture<'_, StorageResult<()>> {
        let result = if self.mode.load(std::sync::atomic::Ordering::SeqCst) == 3 {
            Err(StorageError::Unavailable("secret vector error".into()))
        } else {
            for record in records {
                self.points
                    .lock()
                    .unwrap()
                    .insert((owner.to_string(), record.id.clone()), record.clone());
            }
            Ok(())
        };
        Box::pin(async { result })
    }
    fn similar_search(
        &self,
        _: &UserId,
        _: &[f32],
        _: usize,
    ) -> BoxFuture<'_, StorageResult<Vec<personal_ai_storage::VectorMatch>>> {
        // 故意返回其他所有者的载荷，证明数据库复核独立于向量过滤。
        let result = if self.mode.load(std::sync::atomic::Ordering::SeqCst) == 3 {
            Err(StorageError::Unavailable("secret vector error".into()))
        } else {
            Ok(self
                .points
                .lock()
                .unwrap()
                .values()
                .cloned()
                .map(|record| personal_ai_storage::VectorMatch { record, score: 0.9 })
                .collect())
        };
        Box::pin(async { result })
    }
    fn remove(&self, _: &UserId, _: &[String]) -> BoxFuture<'_, StorageResult<()>> {
        Box::pin(async { unreachable!() })
    }
}
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn indexing_requires_owner_and_csrf_and_batches_can_be_retried() {
    use std::sync::atomic::Ordering;
    let store = Arc::new(MemoryStore::default());
    let owner = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: "index@example.com".into(),
        display_name: "Owner".into(),
    };
    store.save_user(&owner).await.unwrap();
    let token = "c".repeat(64);
    store.sessions.lock().unwrap().insert(
        format!("{:x}", Sha256::digest(token.as_bytes())),
        owner.id.to_string(),
    );
    let cookie = format!("personal_ai_session_v2={token}");
    let dependencies = Arc::new(IndexDependencies::default());
    let indexer = personal_ai_knowledge::index::DocumentIndexer::new(
        dependencies.clone(),
        dependencies.clone(),
        "m".into(),
        2,
    );
    let state = AppState {
        answering: None,
        indexing: Some(Arc::new(Indexing::new(indexer))),
        web_importer: Arc::new(personal_ai_web_import::PublicWebImporter::default()),
        documents: store.clone(),
        store: store.clone(),
        api_token: Arc::from(TOKEN),
        auth: Arc::new(AuthConfig::new(false)),
    };
    let app = router(state.clone());
    let document = StoredDocument {
        summary: DocumentSummary {
            id: Uuid::new_v4().to_string(),
            title: "Index".into(),
            source: "note.md".into(),
            source_type: "markdown".into(),
            tags: vec![],
            created_at_unix_ms: 1,
            chunk_count: 17,
        },
        markdown: "text".into(),
        original_pdf: None,
        original_html: None,
        chunks: (0..17).map(|i| format!("chunk {i}")).collect(),
    };
    store
        .insert_document(&owner.id, "digest", &document)
        .await
        .unwrap();
    let path = format!("/api/documents/{}/index", document.summary.id);
    assert_eq!(
        auth_request(app.clone(), "POST", &path, None, "", true)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        auth_request(app.clone(), "POST", &path, Some(&cookie), "", false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let foreign = StoredDocument {
        summary: DocumentSummary {
            id: Uuid::new_v4().to_string(),
            ..document.summary.clone()
        },
        ..document.clone()
    };
    store
        .insert_document(
            &UserId::new(Uuid::new_v4().to_string()),
            "foreign-digest",
            &foreign,
        )
        .await
        .unwrap();
    let foreign_path = format!("/api/documents/{}/index", foreign.summary.id);
    assert_eq!(
        auth_request(app.clone(), "POST", &foreign_path, Some(&cookie), "", true)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    for suffix in ["?offset=17", "?offset=-1", "?unexpected=1"] {
        assert_eq!(
            auth_request(
                app.clone(),
                "POST",
                &(path.clone() + suffix),
                Some(&cookie),
                "",
                true
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(dependencies.calls.load(Ordering::SeqCst), 0);
    for _ in 0..2 {
        let response = auth_request(app.clone(), "POST", &path, Some(&cookie), "", true).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["indexed_chunks"], 16);
        assert_eq!(body["next_offset"], 16);
    }
    assert_eq!(dependencies.points.lock().unwrap().len(), 16);
    let response = auth_request(
        app.clone(),
        "POST",
        &(path.clone() + "?offset=16"),
        Some(&cookie),
        "",
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert!(body["next_offset"].is_null());
    assert_eq!(dependencies.points.lock().unwrap().len(), 17);
    for (mode, status) in [
        (1, StatusCode::TOO_MANY_REQUESTS),
        (2, StatusCode::BAD_GATEWAY),
        (3, StatusCode::SERVICE_UNAVAILABLE),
    ] {
        dependencies.mode.store(mode, Ordering::SeqCst);
        let response = auth_request(app.clone(), "POST", &path, Some(&cookie), "", true).await;
        assert_eq!(response.status(), status);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("secret"));
    }
    let disabled = router(AppState {
        answering: None,
        indexing: None,
        ..state
    });
    assert_eq!(
        auth_request(disabled, "POST", &path, Some(&cookie), "", true)
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

async fn retrieval_fixture() -> (
    AppState,
    Arc<MemoryStore>,
    Arc<IndexDependencies>,
    UserId,
    String,
    StoredDocument,
) {
    let store = Arc::new(MemoryStore::default());
    let owner = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: "search@example.com".into(),
        display_name: "Owner".into(),
    };
    store.save_user(&owner).await.unwrap();
    let token = "d".repeat(64);
    store.sessions.lock().unwrap().insert(
        format!("{:x}", Sha256::digest(token.as_bytes())),
        owner.id.to_string(),
    );
    let cookie = format!("personal_ai_session_v2={token}");
    let dependencies = Arc::new(IndexDependencies::default());
    let indexer = personal_ai_knowledge::index::DocumentIndexer::new(
        dependencies.clone(),
        dependencies.clone(),
        "m".into(),
        2,
    );
    let document = StoredDocument {
        summary: DocumentSummary {
            id: Uuid::new_v4().to_string(),
            title: "Verified title".into(),
            source: "local.md".into(),
            source_type: "markdown".into(),
            tags: vec![],
            created_at_unix_ms: 1,
            chunk_count: 1,
        },
        markdown: "verified evidence".into(),
        original_pdf: None,
        original_html: None,
        chunks: vec!["verified evidence".into()],
    };
    store
        .insert_document(&owner.id, "d", &document)
        .await
        .unwrap();
    assert!(indexer.index_batch(&owner.id, &document, 0).await.is_ok());
    let state = AppState {
        answering: None,
        indexing: Some(Arc::new(Indexing::new(indexer))),
        web_importer: Arc::new(personal_ai_web_import::PublicWebImporter::default()),
        documents: store.clone(),
        store: store.clone(),
        api_token: Arc::from(TOKEN),
        auth: Arc::new(AuthConfig::new(false)),
    };
    (state, store, dependencies, owner.id, cookie, document)
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn tools_require_session_csrf_and_server_owned_context() {
    use std::sync::atomic::Ordering;
    let (state, _, dependencies, _, cookie, _) = retrieval_fixture().await;
    let app = router(state.clone());
    let path = "/api/tools/knowledge_search";
    assert_eq!(
        request(
            app.clone(),
            "POST",
            path,
            Some(TOKEN),
            r#"{"query":"evidence"}"#
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    for (session, csrf, expected) in [
        (None, true, StatusCode::UNAUTHORIZED),
        (Some(cookie.as_str()), false, StatusCode::FORBIDDEN),
    ] {
        assert_eq!(
            auth_request(
                app.clone(),
                "POST",
                path,
                session,
                r#"{"query":"evidence"}"#,
                csrf
            )
            .await
            .status(),
            expected
        );
    }
    let manifest = auth_request(app.clone(), "GET", "/api/tools", Some(&cookie), "", false).await;
    assert_eq!(manifest.status(), StatusCode::OK);
    assert_eq!(manifest.headers()[header::CACHE_CONTROL], "no-store");
    let manifest: serde_json::Value =
        serde_json::from_slice(&to_bytes(manifest.into_body(), 16384).await.unwrap()).unwrap();
    assert_eq!(manifest["tools"][0]["name"], "knowledge_search");
    assert_eq!(
        manifest["tools"][0]["input_schema"]["additionalProperties"],
        false
    );
    for input in [
        json!({"query":""}),
        json!({"query":"x","user_id":"forged"}),
        json!({"query":"x","conversation_id":"forged"}),
        json!({"query":"x","limit":6}),
        json!({"query":"x".repeat(1001)}),
        json!([]),
    ] {
        assert_eq!(
            auth_request(
                app.clone(),
                "POST",
                path,
                Some(&cookie),
                &input.to_string(),
                true
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(dependencies.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            "/api/tools/shell",
            Some(&cookie),
            "{}",
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let response = auth_request(
        app.clone(),
        "POST",
        path,
        Some(&cookie),
        r#"{"query":"evidence"}"#,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
    assert_eq!(body["output"]["hits"][0]["text"], "verified evidence");
    let indexing = state.indexing.as_ref().unwrap();
    let permit = indexing.slots.acquire_many(2).await.unwrap();
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            path,
            Some(&cookie),
            r#"{"query":"evidence"}"#,
            true
        )
        .await
        .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(permit);
    dependencies.mode.store(2, Ordering::SeqCst);
    let response = auth_request(
        app,
        "POST",
        path,
        Some(&cookie),
        r#"{"query":"evidence"}"#,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = to_bytes(response.into_body(), 16384).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
        json!({"error":{"code":"tool_failed"}})
    );
    let disabled = router(AppState {
        indexing: None,
        ..state
    });
    let manifest = auth_request(
        disabled.clone(),
        "GET",
        "/api/tools",
        Some(&cookie),
        "",
        false,
    )
    .await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &to_bytes(manifest.into_body(), 16384).await.unwrap()
        )
        .unwrap(),
        json!({"tools":[]})
    );
    assert_eq!(
        auth_request(
            disabled,
            "POST",
            path,
            Some(&cookie),
            r#"{"query":"evidence"}"#,
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn search_revalidates_untrusted_vectors_and_rejects_unauthorized_and_invalid_requests() {
    use std::sync::atomic::Ordering;
    let (state, store, dependencies, owner, cookie, document) = retrieval_fixture().await;
    let app = router(state.clone());
    let path = "/api/knowledge/search";
    for (session, csrf, status) in [
        (None, true, StatusCode::UNAUTHORIZED),
        (Some(cookie.as_str()), false, StatusCode::FORBIDDEN),
    ] {
        assert_eq!(
            auth_request(
                app.clone(),
                "POST",
                path,
                session,
                r#"{"query":"evidence"}"#,
                csrf
            )
            .await
            .status(),
            status
        );
    }
    for input in [
        json!({"query":""}),
        json!({"query":" ","limit":1}),
        json!({"query":"x","limit":0}),
        json!({"query":"x","limit":21}),
        json!({"query":"x".repeat(1001)}),
        json!({"query":"x","owner":"forged"}),
    ] {
        assert_eq!(
            auth_request(
                app.clone(),
                "POST",
                path,
                Some(&cookie),
                &input.to_string(),
                true
            )
            .await
            .status(),
            if input.get("owner").is_some() {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::BAD_REQUEST
            }
        );
    }
    assert_eq!(dependencies.calls.load(Ordering::SeqCst), 1);
    let foreign_owner = UserId::new(Uuid::new_v4().to_string());
    let foreign = StoredDocument {
        summary: DocumentSummary {
            id: Uuid::new_v4().to_string(),
            title: "PRIVATE".into(),
            ..document.summary.clone()
        },
        chunks: vec!["PRIVATE".into()],
        ..document.clone()
    };
    store
        .insert_document(&foreign_owner, "foreign", &foreign)
        .await
        .unwrap();
    assert!(
        state
            .indexing
            .as_ref()
            .unwrap()
            .indexer
            .index_batch(&foreign_owner, &foreign, 0)
            .await
            .is_ok()
    );
    {
        let mut points = dependencies.points.lock().unwrap();
        let record = points
            .get_mut(&(owner.to_string(), format!("{}:0", document.summary.id)))
            .unwrap();
        record.source = "FORGED SOURCE".into();
        let mut obsolete = record.clone();
        obsolete.id = format!("{}:1", document.summary.id);
        obsolete.ordinal = 1;
        obsolete.text = "STALE".into();
        points.insert((owner.to_string(), obsolete.id.clone()), obsolete);
    }
    let response = auth_request(
        app.clone(),
        "POST",
        path,
        Some(&cookie),
        r#"{"query":"evidence"}"#,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body = to_bytes(response.into_body(), 16384).await.unwrap();
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(result["hits"].as_array().unwrap().len(), 1);
    assert_eq!(result["hits"][0]["source"], "local.md");
    assert_eq!(result["hits"][0]["text"], "verified evidence");
    for secret in ["PRIVATE", "STALE", "FORGED"] {
        assert!(!String::from_utf8_lossy(&body).contains(secret));
    }
    for (mode, status) in [
        (1, StatusCode::TOO_MANY_REQUESTS),
        (2, StatusCode::BAD_GATEWAY),
        (3, StatusCode::SERVICE_UNAVAILABLE),
    ] {
        dependencies.mode.store(mode, Ordering::SeqCst);
        let response = auth_request(
            app.clone(),
            "POST",
            path,
            Some(&cookie),
            r#"{"query":"evidence"}"#,
            true,
        )
        .await;
        assert_eq!(response.status(), status);
        assert!(
            !String::from_utf8_lossy(&to_bytes(response.into_body(), 4096).await.unwrap())
                .contains("secret")
        );
    }
    dependencies.mode.store(0, Ordering::SeqCst);
    dependencies.points.lock().unwrap().clear();
    let response = auth_request(
        app,
        "POST",
        path,
        Some(&cookie),
        r#"{"query":"missing"}"#,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let result: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(result["hits"], json!([]));
}

#[derive(Default)]
struct AnswerFixture {
    calls: std::sync::atomic::AtomicUsize,
    mode: std::sync::atomic::AtomicUsize,
}
impl personal_ai_llm::AnswerProvider for AnswerFixture {
    fn answer(
        &self,
        _: &str,
        sources: &[personal_ai_llm::AnswerSource],
    ) -> personal_ai_llm::BoxFuture<'_, personal_ai_llm::LlmResult<personal_ai_llm::ModelAnswer>>
    {
        use personal_ai_llm::{LlmError, ModelAnswer};
        use std::sync::atomic::Ordering;
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].text, "verified evidence");
        let result = match self.mode.load(Ordering::SeqCst) {
            1 => Err(LlmError::RateLimited),
            2 => Err(LlmError::ProviderUnavailable("secret upstream body".into())),
            3 => Ok(ModelAnswer {
                answer: "Forged".into(),
                citations: vec![99],
                insufficient_evidence: false,
            }),
            4 => Ok(ModelAnswer {
                answer: String::new(),
                citations: vec![],
                insufficient_evidence: true,
            }),
            5 => Ok(ModelAnswer {
                answer: "Uncited".into(),
                citations: vec![],
                insufficient_evidence: false,
            }),
            6 => Ok(ModelAnswer {
                answer: "Contradiction".into(),
                citations: vec![1],
                insufficient_evidence: true,
            }),
            7 => Ok(ModelAnswer {
                answer: "Duplicate".into(),
                citations: vec![1, 1],
                insufficient_evidence: false,
            }),
            8 => Ok(ModelAnswer {
                answer: "Zero".into(),
                citations: vec![0],
                insufficient_evidence: false,
            }),
            9 => Ok(ModelAnswer {
                answer: "x".repeat(4001),
                citations: vec![1],
                insufficient_evidence: false,
            }),
            _ => Ok(ModelAnswer {
                answer: "Supported answer".into(),
                citations: vec![1],
                insufficient_evidence: false,
            }),
        };
        Box::pin(async { result })
    }
}
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn answers_require_verified_evidence_and_never_call_chat_for_empty_results() {
    use std::sync::atomic::Ordering;
    let (mut state, _, dependencies, _, cookie, document) = retrieval_fixture().await;
    let path = "/api/knowledge/answer";
    assert_eq!(
        auth_request(
            router(state.clone()),
            "POST",
            path,
            Some(&cookie),
            r#"{"query":"question"}"#,
            true
        )
        .await
        .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let provider = Arc::new(AnswerFixture::default());
    state.answering = Some(provider.clone());
    let app = router(state.clone());
    for (session, csrf, status) in [
        (None, true, StatusCode::UNAUTHORIZED),
        (Some(cookie.as_str()), false, StatusCode::FORBIDDEN),
    ] {
        assert_eq!(
            auth_request(
                app.clone(),
                "POST",
                path,
                session,
                r#"{"query":"question"}"#,
                csrf
            )
            .await
            .status(),
            status
        );
    }
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            path,
            Some(&cookie),
            r#"{"query":""}"#,
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    let permits = state
        .indexing
        .as_ref()
        .unwrap()
        .slots
        .acquire_many(2)
        .await
        .unwrap();
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            path,
            Some(&cookie),
            r#"{"query":"question"}"#,
            true
        )
        .await
        .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    drop(permits);
    let response = auth_request(
        app.clone(),
        "POST",
        path,
        Some(&cookie),
        r#"{"query":"question"}"#,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
    assert_eq!(body["status"], "answered");
    assert_eq!(body["citations"][0]["id"], 1);
    assert_eq!(body["citations"][0]["document_id"], document.summary.id);
    assert_eq!(body["citations"][0]["text"], "verified evidence");
    for (mode, status) in [
        (1, StatusCode::TOO_MANY_REQUESTS),
        (2, StatusCode::BAD_GATEWAY),
        (3, StatusCode::BAD_GATEWAY),
        (5, StatusCode::BAD_GATEWAY),
        (6, StatusCode::BAD_GATEWAY),
        (7, StatusCode::BAD_GATEWAY),
        (8, StatusCode::BAD_GATEWAY),
        (9, StatusCode::BAD_GATEWAY),
    ] {
        provider.mode.store(mode, Ordering::SeqCst);
        let response = auth_request(
            app.clone(),
            "POST",
            path,
            Some(&cookie),
            r#"{"query":"question"}"#,
            true,
        )
        .await;
        assert_eq!(response.status(), status);
        assert!(
            !String::from_utf8_lossy(&to_bytes(response.into_body(), 4096).await.unwrap())
                .contains("secret")
        );
    }
    provider.mode.store(4, Ordering::SeqCst);
    let response = auth_request(
        app.clone(),
        "POST",
        path,
        Some(&cookie),
        r#"{"query":"question"}"#,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["status"], "insufficient_evidence");
    dependencies.points.lock().unwrap().clear();
    let calls = provider.calls.load(Ordering::SeqCst);
    let response = auth_request(
        app,
        "POST",
        path,
        Some(&cookie),
        r#"{"query":"missing"}"#,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["status"], "insufficient_evidence");
    assert!(body["answer"].is_null());
    assert_eq!(body["citations"], json!([]));
    assert_eq!(provider.calls.load(Ordering::SeqCst), calls);
}
