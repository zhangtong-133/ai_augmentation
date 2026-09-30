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
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/tools/knowledge_search")
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
