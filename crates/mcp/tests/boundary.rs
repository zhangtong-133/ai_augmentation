use personal_ai_mcp::{bridge::Bridge, protocol::Session};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn bridge(url: &str, enabled: bool) -> Bridge {
    Bridge::new(url, &"a".repeat(64), enabled).unwrap()
}
async fn send(session: &mut Session, id: u32, method: &str, params: Value) -> Value {
    session
        .handle(
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                .to_string()
                .as_bytes(),
        )
        .await
        .unwrap()
}
async fn initialize(session: &mut Session) {
    let reply = send(session, 1, "initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}})).await;
    assert_eq!(reply["result"]["protocolVersion"], "2025-11-25");
    assert!(
        session
            .handle(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await
            .is_none()
    );
}
fn call() -> Value {
    json!({"name":"knowledge_search","arguments":{"request_id":"a3bd4b8d-1b9e-4a1a-9db2-96d34a0a458a","query":"test","acknowledge_embedding_cost":true}})
}
#[tokio::test]
async fn default_off_lifecycle_and_invalid_messages_do_not_dispatch() {
    let mut session = Session::new(bridge("http://127.0.0.1:1", false));
    assert_eq!(
        send(&mut session, 0, "tools/list", json!({})).await["error"]["code"],
        -32000
    );
    initialize(&mut session).await;
    assert_eq!(
        send(&mut session, 2, "tools/list", json!({})).await["result"]["tools"],
        json!([])
    );
    assert_eq!(
        send(&mut session, 3, "tools/call", call()).await["result"]["content"][0]["text"],
        "embedding_cost_not_enabled"
    );
    assert_eq!(
        send(&mut session, 3, "tools/list", json!({})).await["error"]["code"],
        -32600
    );
    let mut forged = call();
    forged["arguments"]["user_id"] = json!("someone-else");
    assert_eq!(
        send(&mut session, 4, "tools/call", forged).await["error"]["code"],
        -32602
    );
    let mut unpaid = call();
    unpaid["arguments"]["acknowledge_embedding_cost"] = json!(false);
    assert_eq!(
        send(&mut session, 5, "tools/call", unpaid).await["error"]["code"],
        -32602
    );
    assert_eq!(
        session.handle(b"[]").await.unwrap()["error"]["code"],
        -32600
    );
    assert_eq!(session.handle(b"{").await.unwrap()["error"]["code"], -32700);
    assert!(
        session
            .handle(
                json!({"jsonrpc":"2.0","method":"tools/call","params":call()})
                    .to_string()
                    .as_bytes()
            )
            .await
            .is_none()
    );
}
#[test]
fn credentials_and_origin_are_restricted() {
    for url in [
        "https://127.0.0.1",
        "http://example.com",
        "http://localhost",
        "http://user@127.0.0.1",
        "http://127.0.0.1/path",
        "http://127.0.0.1?x=1",
    ] {
        assert!(Bridge::new(url, &"a".repeat(64), true).is_err());
    }
    assert!(Bridge::new("http://127.0.0.1", "invalid\r\nheader", true).is_err());
}
#[tokio::test]
async fn bridge_forwards_only_fixed_credentials_and_maps_duplicate_without_retry() {
    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let app = Router::new()
        .route("/api/tools", get(|| async { Json(json!({"tools":[{"name":"knowledge_search","read_only":true}]})) }))
        .route("/api/tools/knowledge_search", post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let calls = calls.clone();
            async move {
                assert_eq!(headers["cookie"], format!("personal_ai_session_v2={}", "a".repeat(64)));
                assert_eq!(headers["x-requested-with"], "personal-ai");
                assert_eq!(headers["idempotency-key"], "a3bd4b8d-1b9e-4a1a-9db2-96d34a0a458a");
                assert_eq!(body, json!({"query":"test","limit":5}));
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    (StatusCode::OK, Json(json!({"tool":"knowledge_search","output":{"hits":[{"text":"untrusted text"}]},"private":"secret"})))
                } else {
                    (StatusCode::CONFLICT, Json(json!({"private":"secret"})))
                }
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut session = Session::new(bridge(&url, true));
    initialize(&mut session).await;
    assert_eq!(
        send(&mut session, 2, "tools/list", json!({})).await["result"]["tools"][0]["name"],
        "knowledge_search"
    );
    let success = send(&mut session, 3, "tools/call", call()).await;
    assert_eq!(
        success["result"]["structuredContent"]["hits"][0]["text"],
        "untrusted text"
    );
    assert!(!success.to_string().contains("secret"));
    let duplicate = send(&mut session, 4, "tools/call", call()).await;
    assert_eq!(duplicate["result"]["isError"], true);
    assert!(!duplicate.to_string().contains("secret"));
    assert_eq!(count.load(Ordering::SeqCst), 2);
    server.abort();
}
#[test]
fn stdio_emits_only_json_and_bounds_frames() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_personal-ai-mcp"))
        .env("MCP_API_URL", "http://127.0.0.1:1")
        .env("MCP_SESSION_TOKEN", "a".repeat(64))
        .env("MCP_ALLOW_EMBEDDING_COST", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"{}\n{\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(text.lines().count(), 2);
    for line in text.lines() {
        assert_eq!(
            serde_json::from_str::<Value>(line).unwrap()["jsonrpc"],
            "2.0"
        );
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_personal-ai-mcp"))
        .env("MCP_API_URL", "http://127.0.0.1:1")
        .env("MCP_SESSION_TOKEN", "a".repeat(64))
        .env("MCP_ALLOW_EMBEDDING_COST", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&vec![b'x'; 16_385])
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "MCP input frame exceeds 16 KiB"
    );
}

#[tokio::test]
async fn failures_are_sanitized_without_redirects_or_retries() {
    use axum::{
        Router,
        http::{StatusCode, header::LOCATION},
        routing::post,
    };
    for (status, expected) in [
        (StatusCode::UNAUTHORIZED, "session_expired"),
        (StatusCode::TOO_MANY_REQUESTS, "tool_limit_reached"),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "api_unavailable_or_result_unknown",
        ),
        (
            StatusCode::TEMPORARY_REDIRECT,
            "api_unavailable_or_result_unknown",
        ),
        (StatusCode::OK, "api_response_too_large"),
    ] {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let app = Router::new().route(
            "/api/tools/knowledge_search",
            post(move || {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    (
                        status,
                        [(LOCATION, "/api/tools/knowledge_search")],
                        "private".repeat(25_000),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut session = Session::new(bridge(&url, true));
        initialize(&mut session).await;
        let reply = send(&mut session, 2, "tools/call", call()).await;
        assert_eq!(reply["result"]["content"][0]["text"], expected);
        assert!(!reply.to_string().contains("private"));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
