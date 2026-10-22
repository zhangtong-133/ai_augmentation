use super::*;
use crate::{MAX_LINE, MAX_OUTPUT, MAX_WIRE};
use axum::{http::StatusCode, response::IntoResponse};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

const OUTPUT: &str = r#"{"answer":"答案🙂","citations":[{"id":1,"quote":"唯一证据"}],"insufficient_evidence":false}"#;

fn sources() -> Vec<AnswerSource> {
    vec![AnswerSource {
        id: 1,
        text: "前🙂言：唯一证据".into(),
    }]
}
fn record(text: &str, done: bool) -> Value {
    let mut value =
        json!({"model":"fixture", "message":{"role":"assistant","content":text}, "done":done});
    if done {
        value["done_reason"] = json!("stop");
    }
    value
}
fn line(value: &Value) -> String {
    format!("{value}\n")
}
fn wire(output: &str) -> String {
    line(&record(output, false)) + &line(&record("", true))
}

struct Case {
    name: &'static str,
    body: String,
    status: StatusCode,
    content_type: &'static str,
    valid: bool,
}
fn cases() -> Vec<Case> {
    let mut cases: Vec<_> = [
        ("answer", wire(OUTPUT), true),
        (
            "insufficient",
            wire(r#"{"answer":"","citations":[],"insufficient_evidence":true}"#),
            true,
        ),
        ("malformed answer", wire("secret"), false),
        ("unknown answer fields", wire(r#"{"secret":true}"#), false),
        ("missing done", line(&record(OUTPUT, false)), false),
        (
            "missing final newline",
            wire(OUTPUT).trim_end().into(),
            false,
        ),
        (
            "late record",
            wire(OUTPUT) + &line(&record("", true)),
            false,
        ),
        (
            "late error",
            wire(OUTPUT) + "{\"error\":\"secret\"}\n",
            false,
        ),
        ("empty answer stream", line(&record("", true)), false),
        ("malformed record", "secret\n".into(), false),
        ("oversized line", "x".repeat(MAX_LINE + 1), false),
        (
            "oversized wire",
            line(&record("", false)).repeat(MAX_WIRE / line(&record("", false)).len() + 1),
            false,
        ),
        (
            "oversized output",
            line(&record(&"x".repeat(MAX_OUTPUT + 1), true)),
            false,
        ),
    ]
    .into_iter()
    .map(|(name, body, valid)| Case {
        name,
        body,
        status: StatusCode::OK,
        content_type: "application/x-ndjson; charset=utf-8",
        valid,
    })
    .collect();
    for (name, path, value) in [
        ("wrong model", vec!["model"], json!("other")),
        ("truncated generation", vec!["done_reason"], json!("length")),
        ("wrong role", vec!["message", "role"], json!("user")),
        ("reasoning", vec!["message", "thinking"], json!("secret")),
        ("tools", vec!["message", "tool_calls"], json!([{}])),
        ("error", vec!["error"], json!("secret")),
        ("remote model", vec!["remote_model"], json!("other:cloud")),
        (
            "remote host",
            vec!["remote_host"],
            json!("https://example.com"),
        ),
    ] {
        let mut bad = record(OUTPUT, true);
        let mut field = &mut bad;
        for key in path {
            field = &mut field[key];
        }
        *field = value;
        cases.push(Case {
            name,
            body: line(&bad),
            status: StatusCode::OK,
            content_type: "application/x-ndjson",
            valid: false,
        });
    }
    cases.extend([
        Case {
            name: "wrong content type",
            body: wire(OUTPUT),
            status: StatusCode::OK,
            content_type: "application/json",
            valid: false,
        },
        Case {
            name: "HTTP unavailable",
            body: "secret".into(),
            status: StatusCode::SERVICE_UNAVAILABLE,
            content_type: "application/x-ndjson",
            valid: false,
        },
        Case {
            name: "redirect is not followed",
            body: "secret".into(),
            status: StatusCode::TEMPORARY_REDIRECT,
            content_type: "application/x-ndjson",
            valid: false,
        },
    ]);
    cases
}

#[tokio::test]
async fn native_http_sends_the_exact_preview_once_and_rejects_invalid_streams() {
    for case in cases() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = LocalTarget::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            "fixture",
        )
        .unwrap();
        let expected = preview_for(AnswerBackend::Ollama, &target, "question", &sources())
            .unwrap()
            .body()
            .clone();
        let expected_bytes = serde_json::to_vec(&expected).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let app = axum::Router::new().route(
            "/api/chat",
            axum::routing::post(
                move |headers: axum::http::HeaderMap, input: axum::body::Bytes| {
                    let count = count.clone();
                    let expected = expected.clone();
                    let expected_bytes = expected_bytes.clone();
                    let body = case.body.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        assert!(!headers.contains_key("authorization"));
                        assert!(!headers.contains_key("cookie"));
                        assert_eq!(input.as_ref(), expected_bytes);
                        assert_eq!(serde_json::from_slice::<Value>(&input).unwrap(), expected);
                        (
                            case.status,
                            [
                                ("content-type", case.content_type),
                                ("location", "/api/chat"),
                            ],
                            body,
                        )
                            .into_response()
                    }
                },
            ),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = OllamaLocalAnswers::new(target).unwrap();
        let result = provider.answer("question", &sources()).await;
        assert_eq!(result.is_ok(), case.valid, "{}", case.name);
        if let Ok(answer) = result {
            if case.name == "answer" {
                assert_eq!(answer.answer, "答案🙂");
                assert_eq!(answer.citations[0].quote, "唯一证据");
            } else {
                assert!(answer.insufficient_evidence);
                assert_eq!(answer.answer, "");
                assert_eq!(answer.citations, []);
            }
        } else if let Err(error) = result {
            assert!(!error.to_string().contains("secret"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{}", case.name);
        assert!(provider.answer(" ", &sources()).await.is_err());
        let oversized: Vec<_> = (1..=5)
            .map(|id| AnswerSource {
                id,
                text: "🙂".repeat(1000),
            })
            .collect();
        assert!(provider.answer("question", &oversized).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{}", case.name);
        server.abort();
    }
}

#[tokio::test]
async fn dropping_a_pending_request_does_not_retry_or_send_a_second_request() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = LocalTarget::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        "fixture",
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let notified = started.clone();
    let app = axum::Router::new().route(
        "/api/chat",
        axum::routing::post(move || {
            let count = count.clone();
            let notified = notified.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                notified.notify_one();
                std::future::pending::<()>().await;
                ""
            }
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let provider = OllamaLocalAnswers::new(target).unwrap();
    let request = tokio::spawn(async move { provider.answer("question", &sources()).await });
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.abort();
}
