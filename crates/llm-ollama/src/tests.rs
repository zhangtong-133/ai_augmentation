use super::*;
use personal_ai_llm::{ChatMessage, stream::IgnoreTextDeltas};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
fn line(content: &str, done: bool) -> String {
    format!(
        "{}\n",
        json!({"model":"qwen3:4b", "done":done, "done_reason":if done {Some("stop")} else {None}, "message":{"role":"assistant", "content":content}})
    )
}
fn request() -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            content: "fixture".into(),
        }],
        temperature: None,
        max_output_tokens: None,
    }
}
#[test]
fn split_utf8_and_terminal_require_clean_eof() {
    let wire = line("{\"理由\":\"测试\"}", false) + &line("", true);
    for split in 0..wire.len() {
        let mut parser = Parser::new("qwen3:4b");
        parser
            .push(&wire.as_bytes()[..split], &IgnoreTextDeltas)
            .unwrap();
        parser
            .push(&wire.as_bytes()[split..], &IgnoreTextDeltas)
            .unwrap();
        assert_eq!(parser.finish().unwrap(), "{\"理由\":\"测试\"}");
    }
    let mut parser = Parser::new("qwen3:4b");
    parser
        .push(line("partial", false).as_bytes(), &IgnoreTextDeltas)
        .unwrap();
    assert!(parser.finish().is_err());
}
#[test]
fn truncation_wrong_model_length_tools_and_late_error_are_rejected() {
    for wire in [
        line("ok", true) + "{\"error\":\"late\"}\n",
        line("ok", true) + &line("", true),
        line("ok", true).replace("qwen3:4b", "other"),
        line("ok", true).replace("stop", "length"),
        line("ok", true).replace("\"content\":", "\"tool_calls\":[{}],\"content\":"),
        line("ok", true).trim_end().into(),
        line("x", false) + "invalid",
    ] {
        let mut parser = Parser::new("qwen3:4b");
        assert!(
            parser
                .push(wire.as_bytes(), &IgnoreTextDeltas)
                .and_then(|()| parser.finish())
                .is_err()
        );
    }
    let mut parser = Parser::new("qwen3:4b");
    assert!(
        parser
            .push(
                line(&"x".repeat(MAX_OUTPUT + 1), true).as_bytes(),
                &IgnoreTextDeltas
            )
            .is_err()
    );
}
async fn server(
    body: String,
    delay: Duration,
    calls: Arc<AtomicUsize>,
) -> (LocalTarget, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = LocalTarget::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        "qwen3:4b",
    )
    .unwrap();
    let app = axum::Router::new().route(
        "/api/chat",
        axum::routing::post(move |axum::Json(input): axum::Json<serde_json::Value>| {
            let body = body.clone();
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(input["keep_alive"], 0);
                assert_eq!(input["think"], false);
                assert_eq!(input["format"], "json");
                assert_eq!(input["options"]["num_ctx"], CONTEXT_TOKENS);
                assert!(input.get("tools").is_none());
                tokio::time::sleep(delay).await;
                ([("content-type", "application/x-ndjson")], body)
            }
        }),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (target, task)
}
#[tokio::test]
async fn http_sends_once_and_does_not_retry_invalid_output() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (target, task) = server(
        line("{}", true) + "{\"error\":\"late\"}\n",
        Duration::ZERO,
        calls.clone(),
    )
    .await;
    assert!(
        Ollama::new()
            .unwrap()
            .infer(&target, &request(), &IgnoreTextDeltas)
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
}
#[tokio::test]
async fn http_returns_only_complete_output_and_rejects_oversize_before_send() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (target, task) = server(
        line("{", false) + &line("}", true),
        Duration::ZERO,
        calls.clone(),
    )
    .await;
    let client = Ollama::new().unwrap();
    assert_eq!(
        client
            .infer(&target, &request(), &IgnoreTextDeltas)
            .await
            .unwrap(),
        "{}"
    );
    let mut huge = request();
    huge.messages[0].content = "x".repeat(MAX_PROMPT_BYTES + 1);
    assert!(
        client
            .infer(&target, &huge, &IgnoreTextDeltas)
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
}
#[tokio::test]
async fn caller_timeout_or_cancel_never_resends() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (target, task) = server(line("{}", true), Duration::from_secs(2), calls.clone()).await;
    let client = Ollama::new().unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(50),
            client.infer(&target, &request(), &IgnoreTextDeltas)
        )
        .await
        .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let target2 = target.clone();
    let inference =
        tokio::spawn(async move { client.infer(&target2, &request(), &IgnoreTextDeltas).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    inference.abort();
    assert!(inference.await.unwrap_err().is_cancelled());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    task.abort();
}
