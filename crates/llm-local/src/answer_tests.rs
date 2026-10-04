use super::*;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
fn sources() -> Vec<AnswerSource> {
    vec![AnswerSource {
        id: 1,
        text: "前🙂言：唯一证据".into(),
    }]
}
fn wire(output: &str, finish: &str) -> String {
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"model":"fixture-local", "choices":[{"index":0,"delta":{"content":output},"finish_reason":finish}]})
    )
}
#[test]
fn previews_bind_wire_target_model_and_local_byte_budget() {
    let target = LocalTarget::new("http://127.0.0.1:11435", "fixture-local").unwrap();
    let value = preview(&target, "question", &sources()).unwrap();
    assert_eq!(value.body()["stream"], true);
    assert_eq!(value.body()["temperature"], 0.0);
    assert_eq!(value.body()["max_tokens"], 2048);
    assert_eq!(value.body()["n"], 1);
    assert!(value.body().get("tools").is_none());
    let digest = value.fingerprint().unwrap();
    for changed in [
        LocalTarget::new("http://127.0.0.1:11436", "fixture-local").unwrap(),
        LocalTarget::new("http://127.0.0.1:11435", "other").unwrap(),
    ] {
        assert_ne!(
            digest,
            preview(&changed, "question", &sources())
                .unwrap()
                .fingerprint()
                .unwrap()
        );
    }
    assert_ne!(
        digest,
        preview(&target, "question ", &sources())
            .unwrap()
            .fingerprint()
            .unwrap()
    );
    let large: Vec<_> = (1..=5)
        .map(|id| AnswerSource {
            id,
            text: "🙂".repeat(1000),
        })
        .collect();
    assert!(prepare("question", &large).is_ok());
    assert!(preview(&target, "question", &large).is_err());
}
#[tokio::test]
async fn actual_body_matches_preview_and_only_clean_completed_json_is_returned() {
    for (output, finish, late, valid) in [
        (
            r#"{"answer":"答案","citations":[{"id":1,"quote":"唯一证据"}],"insufficient_evidence":false}"#,
            "stop",
            false,
            true,
        ),
        (
            r#"{"answer":"","citations":[],"insufficient_evidence":true}"#,
            "stop",
            false,
            true,
        ),
        (
            r#"{"answer":"secret","citations":[1],"insufficient_evidence":false}"#,
            "stop",
            false,
            false,
        ),
        ("secret", "stop", false, false),
        ("{}", "length", false, false),
        ("{}", "stop", true, false),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = LocalTarget::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            "fixture-local",
        )
        .unwrap();
        let expected = preview(&target, "question", &sources())
            .unwrap()
            .body()
            .clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let body = wire(output, finish) + if late { "{\"error\":\"secret\"}\n" } else { "" };
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(
                move |headers: axum::http::HeaderMap,
                      axum::Json(input): axum::Json<serde_json::Value>| {
                    let count = count.clone();
                    let expected = expected.clone();
                    let body = body.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        assert!(!headers.contains_key("authorization"));
                        assert_eq!(input, expected);
                        ([("content-type", "text/event-stream")], body)
                    }
                },
            ),
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let provider = LocalAnswers::new(target).unwrap();
        let result = provider.answer("question", &sources()).await;
        assert_eq!(result.is_ok(), valid);
        if let Err(error) = result {
            assert!(!error.to_string().contains("secret"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(provider.answer(" ", &sources()).await.is_err());
        let large: Vec<_> = (1..=5)
            .map(|id| AnswerSource {
                id,
                text: "🙂".repeat(1000),
            })
            .collect();
        assert!(provider.answer("question", &large).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        task.abort();
    }
}
