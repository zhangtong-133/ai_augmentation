use super::*;
use personal_ai_storage::tool_calls::{
    NewToolCall, ToolCall, ToolCallAudit, ToolCallFinish, ToolCallStart, ToolCallStore,
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
pub(super) struct MemoryToolCalls {
    calls: Mutex<HashMap<(String, String), (NewToolCall, ToolCall)>>,
    mode: AtomicUsize,
}
impl ToolCallStore for MemoryToolCalls {
    fn start_tool_call(
        &self,
        owner: &UserId,
        input: &NewToolCall,
    ) -> BoxFuture<'_, StorageResult<ToolCallStart>> {
        let result = {
            let mut calls = self.calls.lock().unwrap();
            let key = (owner.to_string(), input.request_id.clone());
            if self.mode.load(Ordering::SeqCst) == 1 {
                Err(StorageError::Unavailable("private database failure".into()))
            } else if let Some((old, call)) = calls.get(&key) {
                if old.arguments_digest != input.arguments_digest || old.tool != input.tool {
                    Err(StorageError::Conflict("request already used".into()))
                } else {
                    Ok(ToolCallStart::Existing(call.clone()))
                }
            } else if self.mode.load(Ordering::SeqCst) == 3 {
                Err(StorageError::Conflict("tool call quota reached".into()))
            } else {
                let call = ToolCall {
                    request_id: input.request_id.clone(),
                    tool: input.tool.clone(),
                    day: "2026-09-30".into(),
                    status: "running".into(),
                    input_bytes: input.input_bytes,
                    output_bytes: None,
                    created_at_unix_ms: 0,
                    deadline_unix_ms: 60_000,
                    finished_at_unix_ms: None,
                };
                calls.insert(key, (input.clone(), call.clone()));
                Ok(ToolCallStart::Started(call))
            }
        };
        Box::pin(async { result })
    }
    fn finish_tool_call(
        &self,
        owner: &UserId,
        id: &str,
        finish: ToolCallFinish,
    ) -> BoxFuture<'_, StorageResult<ToolCall>> {
        let result = if self.mode.load(Ordering::SeqCst) == 2 {
            Err(StorageError::Unavailable("private database failure".into()))
        } else {
            self.calls
                .lock()
                .unwrap()
                .get_mut(&(owner.to_string(), id.into()))
                .map(|(_, call)| {
                    call.status = finish.outcome.as_str().into();
                    call.output_bytes = finish.output_bytes;
                    call.finished_at_unix_ms = Some(1);
                    call.clone()
                })
                .ok_or(StorageError::NotFound)
        };
        Box::pin(async { result })
    }
    fn get_tool_call(&self, owner: &UserId, id: &str) -> BoxFuture<'_, StorageResult<ToolCall>> {
        let result = self
            .calls
            .lock()
            .unwrap()
            .get(&(owner.to_string(), id.into()))
            .map(|(_, call)| call.clone())
            .ok_or(StorageError::NotFound);
        Box::pin(async { result })
    }
    fn audit_tool_calls(
        &self,
        owner: &UserId,
        day: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ToolCallAudit>> {
        let day = day.unwrap_or("2026-09-30").to_owned();
        let result = personal_ai_storage::dates::validate_utc_day(&day).map(|()| {
            let items: Vec<_> = self
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|((user, _), (_, call))| user == owner.as_str() && call.day == day)
                .map(|(_, (_, call))| call.clone())
                .collect();
            ToolCallAudit {
                day,
                used: i32::try_from(items.len()).unwrap(),
                limit: 100,
                items,
            }
        });
        Box::pin(async { result })
    }
}

async fn call(
    app: Router,
    cookie: Option<&str>,
    id: Option<&str>,
    body: &str,
    csrf: bool,
) -> axum::response::Response {
    named_call(app, "knowledge_search", cookie, id, body, csrf).await
}

async fn named_call(
    app: Router,
    name: &str,
    cookie: Option<&str>,
    id: Option<&str>,
    body: &str,
    csrf: bool,
) -> axum::response::Response {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/api/tools/{name}"))
        .header("content-type", "application/json");
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    if let Some(id) = id {
        request = request.header("idempotency-key", id);
    }
    if csrf {
        request = request.header("x-requested-with", "personal-ai");
    }
    app.oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

async fn value(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap()
}

#[tokio::test]
async fn audited_tool_http_requires_id_and_blocks_replay_and_conflicting_arguments() {
    let (mut state, _, dependencies, _, cookie, _) = retrieval_fixture().await;
    let ledger = Arc::new(MemoryToolCalls::default());
    state.tool_calls = Some(ledger.clone());
    let app = router(state);
    let id = Uuid::new_v4().to_string();
    for key in [None, Some("invalid")] {
        assert_eq!(
            call(
                app.clone(),
                Some(&cookie),
                key,
                r#"{"query":"evidence"}"#,
                true
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(dependencies.calls.load(Ordering::SeqCst), 1);
    let response = call(
        app.clone(),
        Some(&cookie),
        Some(&id),
        r#"{"query":"evidence","limit":5}"#,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body = value(response).await;
    assert_eq!(body["call"]["request_id"], id);
    assert_eq!(body["call"]["status"], "succeeded");
    assert_eq!(dependencies.calls.load(Ordering::SeqCst), 2);
    let repeated = call(
        app.clone(),
        Some(&cookie),
        Some(&id),
        r#"{"limit":5, "query":"evidence"}"#,
        true,
    )
    .await;
    assert_eq!(repeated.status(), StatusCode::CONFLICT);
    assert_eq!(
        value(repeated).await["error"]["code"],
        "tool_call_already_used"
    );
    let conflict = call(
        app.clone(),
        Some(&cookie),
        Some(&id),
        r#"{"query":"changed"}"#,
        true,
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(value(conflict).await["error"]["code"], "tool_call_conflict");
    assert_eq!(dependencies.calls.load(Ordering::SeqCst), 2);
    let response = auth_request(
        app.clone(),
        "GET",
        "/api/tool-calls",
        Some(&cookie),
        "",
        false,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let report = value(response).await;
    assert_eq!(report["used"], 1);
    assert_eq!(report["limit"], 100);
    assert_eq!(report["items"][0]["status"], "succeeded");
    assert!(!report.to_string().contains("verified evidence"));
    assert!(!report.to_string().contains("arguments_digest"));
    for path in [
        "/api/tool-calls".to_owned(),
        format!("/api/tool-calls/{id}"),
    ] {
        assert_eq!(
            auth_request(app.clone(), "GET", &path, None, "", false)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for path in [
        "/api/tool-calls?day=2026-02-29",
        "/api/tool-calls?user_id=forged",
        "/api/tool-calls/invalid",
    ] {
        assert_eq!(
            auth_request(app.clone(), "GET", path, Some(&cookie), "", false)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn audit_storage_failures_fail_closed_and_never_retry_executed_tools() {
    let (mut state, _, dependencies, _, cookie, _) = retrieval_fixture().await;
    let ledger = Arc::new(MemoryToolCalls::default());
    state.tool_calls = Some(ledger.clone());
    let app = router(state);
    let input = r#"{"query":"evidence"}"#;
    for (mode, expected, code) in [
        (1, StatusCode::SERVICE_UNAVAILABLE, "tool_audit_unavailable"),
        (3, StatusCode::TOO_MANY_REQUESTS, "tool_daily_limit"),
    ] {
        ledger.mode.store(mode, Ordering::SeqCst);
        let response = call(
            app.clone(),
            Some(&cookie),
            Some(&Uuid::new_v4().to_string()),
            input,
            true,
        )
        .await;
        assert_eq!(response.status(), expected);
        assert_eq!(value(response).await, json!({"error":{"code":code}}));
        assert_eq!(dependencies.calls.load(Ordering::SeqCst), 1);
    }
    ledger.mode.store(2, Ordering::SeqCst);
    let id = Uuid::new_v4().to_string();
    let response = call(app.clone(), Some(&cookie), Some(&id), input, true).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        value(response).await,
        json!({"error":{"code":"tool_audit_unavailable"}})
    );
    assert_eq!(dependencies.calls.load(Ordering::SeqCst), 2);
    ledger.mode.store(0, Ordering::SeqCst);
    assert_eq!(
        call(app, Some(&cookie), Some(&id), input, true)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(dependencies.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn tool_audits_and_call_ids_are_scoped_by_authenticated_user() {
    let (state, store, _, _, cookie, _) = retrieval_fixture().await;
    let other = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: "other-tool-audit@example.com".into(),
        display_name: "Other".into(),
    };
    store.save_user(&other).await.unwrap();
    let token = "f".repeat(64);
    store.sessions.lock().unwrap().insert(
        format!("{:x}", Sha256::digest(token.as_bytes())),
        other.id.to_string(),
    );
    let other_cookie = format!("personal_ai_session_v2={token}");
    let app = router(state);
    let id = Uuid::new_v4().to_string();
    let input = r#"{"query":"evidence"}"#;
    assert_eq!(
        call(app.clone(), Some(&cookie), Some(&id), input, true)
            .await
            .status(),
        StatusCode::OK
    );
    let path = format!("/api/tool-calls/{id}");
    assert_eq!(
        auth_request(app.clone(), "GET", &path, Some(&other_cookie), "", false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let empty = value(
        auth_request(
            app.clone(),
            "GET",
            "/api/tool-calls",
            Some(&other_cookie),
            "",
            false,
        )
        .await,
    )
    .await;
    assert_eq!(empty["used"], 0);
    assert_eq!(empty["items"], json!([]));
    let response = call(app.clone(), Some(&other_cookie), Some(&id), input, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(value(response).await["output"]["hits"], json!([]));
    for cookie in [cookie, other_cookie] {
        let report = value(
            auth_request(
                app.clone(),
                "GET",
                "/api/tool-calls",
                Some(&cookie),
                "",
                false,
            )
            .await,
        )
        .await;
        assert_eq!(report["used"], 1);
        assert_eq!(report["items"].as_array().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn file_reader_pages_unicode_without_indexing_and_audits_once() {
    let (mut state, store, dependencies, owner, cookie, mut document) = retrieval_fixture().await;
    state.indexing = None;
    let ledger = Arc::new(MemoryToolCalls::default());
    state.tool_calls = Some(ledger.clone());
    document.markdown = "中文🙂abc".into();
    store
        .documents
        .lock()
        .unwrap()
        .get_mut(&document.summary.id)
        .unwrap()
        .2 = document.clone();
    let app = router(state);
    for (offset, expected, next) in [
        (0, "中文", Some(2)),
        (2, "🙂a", Some(4)),
        (4, "bc", None),
        (6, "", None),
    ] {
        let id = Uuid::new_v4().to_string();
        let body = json!({"document_id":document.summary.id,"offset":offset,"limit":2}).to_string();
        let response = named_call(
            app.clone(),
            "file_reader",
            Some(&cookie),
            Some(&id),
            &body,
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let output = value(response).await;
        assert_eq!(output["output"]["text"], expected);
        assert_eq!(output["output"]["next_offset"], json!(next));
        assert_eq!(output["output"]["total_chars"], 6);
        assert_eq!(output["call"]["status"], "succeeded");
        assert_eq!(
            named_call(
                app.clone(),
                "file_reader",
                Some(&cookie),
                Some(&id),
                &body,
                true
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
    }
    assert_eq!(dependencies.calls.load(Ordering::SeqCst), 1); // 只有夹具准备时索引，读取不调用模型。
    let audit = ledger.audit_tool_calls(&owner, None).await.unwrap();
    assert_eq!(audit.used, 4);
    assert!(!serde_json::to_string(&audit).unwrap().contains("中文"));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn file_reader_rejects_untrusted_arguments_and_private_documents_and_honors_quota() {
    let (mut state, store, _, _, cookie, document) = retrieval_fixture().await;
    state.indexing = None;
    let ledger = Arc::new(MemoryToolCalls::default());
    state.tool_calls = Some(ledger.clone());
    let app = router(state);
    let valid = json!({"document_id":document.summary.id});
    let id = Uuid::new_v4().to_string();
    for (session, csrf, status) in [
        (None, true, StatusCode::UNAUTHORIZED),
        (Some(cookie.as_str()), false, StatusCode::FORBIDDEN),
    ] {
        assert_eq!(
            named_call(
                app.clone(),
                "file_reader",
                session,
                Some(&id),
                &valid.to_string(),
                csrf
            )
            .await
            .status(),
            status
        );
    }
    for input in [
        json!({"path":"/etc/passwd"}),
        json!({"document_id":"https://example.com"}),
        json!({"document_id":document.summary.id,"user_id":"forged"}),
        json!({"document_id":document.summary.id,"offset":-1}),
        json!({"document_id":document.summary.id,"offset":2_000_001}),
        json!({"document_id":document.summary.id,"limit":0}),
        json!({"document_id":document.summary.id,"limit":4001}),
    ] {
        assert_eq!(
            named_call(
                app.clone(),
                "file_reader",
                Some(&cookie),
                Some(&id),
                &input.to_string(),
                true
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    for key in [None, Some("invalid")] {
        assert_eq!(
            named_call(
                app.clone(),
                "file_reader",
                Some(&cookie),
                key,
                &valid.to_string(),
                true
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(ledger.calls.lock().unwrap().len(), 0);
    ledger.mode.store(3, Ordering::SeqCst);
    assert_eq!(
        named_call(
            app.clone(),
            "file_reader",
            Some(&cookie),
            Some(&id),
            &valid.to_string(),
            true
        )
        .await
        .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    ledger.mode.store(0, Ordering::SeqCst);
    let beyond = json!({"document_id":document.summary.id,"offset":1000});
    assert_eq!(
        named_call(
            app.clone(),
            "file_reader",
            Some(&cookie),
            Some(&id),
            &beyond.to_string(),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    store
        .documents
        .lock()
        .unwrap()
        .get_mut(&document.summary.id)
        .unwrap()
        .0 = Uuid::new_v4().to_string();
    for document_id in [document.summary.id, Uuid::new_v4().to_string()] {
        let response = named_call(
            app.clone(),
            "file_reader",
            Some(&cookie),
            Some(&Uuid::new_v4().to_string()),
            &json!({"document_id":document_id}).to_string(),
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            value(response).await,
            json!({"error":{"code":"tool_denied"}})
        );
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // 固定工具完整 HTTP 权限、审计及幂等验证。
async fn git_history_requires_session_csrf_and_quota_and_audits_once_without_content() {
    let (mut state, _, _, owner, cookie, _) = retrieval_fixture().await;
    state.indexing = None;
    let ledger = Arc::new(MemoryToolCalls::default());
    state.tool_calls = Some(ledger.clone());
    let path = std::env::temp_dir().join(format!("git-http-{}", Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "private-git-subject",
        ],
    ] {
        let output = std::process::Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .arg("-C")
            .arg(&path)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    state.git_tool = personal_ai_git_local::GitLogTool::from_json(
        &json!([{"owner_id":owner.as_str(),"repository_id":"project","path":path}]).to_string(),
    )
    .unwrap()
    .map(Arc::new);
    let app = router(state);
    let body = json!({"repository_id":"project","limit":1}).to_string();
    let id = Uuid::new_v4().to_string();
    for (session, csrf, expected) in [
        (None, true, StatusCode::UNAUTHORIZED),
        (Some(cookie.as_str()), false, StatusCode::FORBIDDEN),
    ] {
        assert_eq!(
            named_call(app.clone(), "git_log", session, Some(&id), &body, csrf)
                .await
                .status(),
            expected
        );
    }
    assert_eq!(
        named_call(
            app.clone(),
            "git_log",
            Some(&cookie),
            Some(&id),
            r#"{"repository_id":"project","path":"/etc"}"#,
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(ledger.calls.lock().unwrap().len(), 0);
    let response = named_call(
        app.clone(),
        "git_log",
        Some(&cookie),
        Some(&id),
        &body,
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        value(response).await["output"]["commits"][0]["subject"],
        "private-git-subject"
    );
    assert_eq!(
        named_call(
            app.clone(),
            "git_log",
            Some(&cookie),
            Some(&id),
            &body,
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let audit = ledger.audit_tool_calls(&owner, None).await.unwrap();
    assert_eq!(audit.used, 1);
    assert!(
        !serde_json::to_string(&audit)
            .unwrap()
            .contains("private-git-subject")
    );
    let denied = named_call(
        app.clone(),
        "git_log",
        Some(&cookie),
        Some(&Uuid::new_v4().to_string()),
        r#"{"repository_id":"unknown"}"#,
        true,
    )
    .await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    ledger.mode.store(3, Ordering::SeqCst);
    assert_eq!(
        named_call(
            app.clone(),
            "git_log",
            Some(&cookie),
            Some(&Uuid::new_v4().to_string()),
            &body,
            true
        )
        .await
        .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    std::fs::remove_dir_all(path).unwrap();
}
