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
#[allow(clippy::too_many_lines)] // A single end-to-end session lifecycle.
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
    // Malformed attempts consume the budget without expensive password work.
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
