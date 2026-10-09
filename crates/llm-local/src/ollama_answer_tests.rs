use super::*;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
fn sources() -> Vec<AnswerSource> {
    vec![AnswerSource {
        id: 1,
        text: "唯一🙂证据".into(),
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
#[test]
fn native_stream_requires_matching_model_clean_eof_and_no_reasoning_or_tools() {
    let text = "唯一🙂证据";
    let good = line(&record(text, false)) + &line(&record("", true));
    let mut parser = Parser::new("fixture");
    for byte in good.as_bytes() {
        parser.push(&[*byte]).unwrap();
    }
    assert_eq!(parser.finish().unwrap(), text);
    for bad in [
        good.trim_end().to_owned(),
        line(&record(text, false)),
        good.clone() + "\n",
        good.clone() + "{\"error\":\"secret\"}\n",
        line(&record("", true)),
        "not json\n".into(),
    ] {
        let mut parser = Parser::new("fixture");
        assert!(
            parser
                .push(bad.as_bytes())
                .and_then(|()| parser.finish())
                .is_err()
        );
    }
    for (path, value) in [
        (vec!["model"], json!("other")),
        (vec!["done_reason"], json!("length")),
        (vec!["message", "role"], json!("user")),
        (vec!["message", "thinking"], json!("secret")),
        (vec!["message", "tool_calls"], json!([{}])),
        (vec!["error"], json!("secret")),
        (vec!["remote_model"], json!("remote:cloud")),
        (vec!["remote_host"], json!("https://example.com")),
    ] {
        let mut bad = record(text, true);
        let mut field = &mut bad;
        for key in path {
            field = &mut field[key];
        }
        *field = value;
        let mut parser = Parser::new("fixture");
        assert!(parser.push(line(&bad).as_bytes()).is_err());
    }
    for data in [
        vec![b'x'; MAX_LINE + 1],
        vec![b'x'; MAX_WIRE + 1],
        line(&record(&"x".repeat(MAX_OUTPUT + 1), true)).into_bytes(),
    ] {
        assert!(Parser::new("fixture").push(&data).is_err());
    }
}
fn native_responses() -> [(u16, &'static str, String, bool); 8] {
    [
        (
            200,
            "application/x-ndjson",
            line(&record(
                r#"{"decision":"complete","excerpts":["s1u1"]}"#,
                true,
            )),
            true,
        ),
        (
            200,
            "application/x-ndjson",
            line(&record(r#"{"decision":"insufficient"}"#, true)),
            true,
        ),
        (
            200,
            "application/x-ndjson",
            line(&record(
                r#"{"decision":"insufficient","excerpts":["s1u1"]}"#,
                true,
            )),
            false,
        ),
        (
            200,
            "application/x-ndjson",
            line(&record(
                r#"{"answer":"唯一证据","citations":[{"id":1,"quote":"唯一🙂证据"}],"insufficient_evidence":false}"#,
                true,
            )),
            false,
        ),
        (
            200,
            "application/x-ndjson",
            line(&record("{\"secret\":true}", true)),
            false,
        ),
        (200, "application/json", line(&record("{}", true)), false),
        (429, "application/x-ndjson", "secret".into(), false),
        (307, "application/x-ndjson", "secret".into(), false),
    ]
}
#[tokio::test]
async fn exact_native_preview_is_sent_once_without_credentials_or_oversized_input() {
    for (status, content_type, body, valid) in native_responses() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = LocalTarget::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            "fixture",
        )
        .unwrap();
        let expected = preview(&target, "question", &sources())
            .unwrap()
            .body()
            .clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let app = axum::Router::new().route(
            "/api/chat",
            axum::routing::post(
                move |headers: axum::http::HeaderMap, axum::Json(input): axum::Json<Value>| {
                    count.fetch_add(1, Ordering::SeqCst);
                    assert!(!headers.contains_key("authorization"));
                    assert_eq!(input, expected);
                    let body = body.clone();
                    async move {
                        (
                            axum::http::StatusCode::from_u16(status).unwrap(),
                            [
                                ("content-type", content_type),
                                ("location", "http://127.0.0.1:1/api/chat"),
                            ],
                            body,
                        )
                    }
                },
            ),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let provider = OllamaAnswers::new(target).unwrap();
        let result = provider.answer("question", &sources()).await;
        assert_eq!(result.is_ok(), valid);
        if let Err(error) = result {
            assert!(!error.to_string().contains("secret"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let large: Vec<_> = (1..=5)
            .map(|id| AnswerSource {
                id,
                text: "🙂".repeat(1000),
            })
            .collect();
        assert!(provider.answer("question", &large).await.is_err());
        assert!(provider.answer(" ", &sources()).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
#[test]
fn preview_binds_runner_parameters_and_frozen_excerpt_selection_in_messages_and_format() {
    let target = LocalTarget::new("http://127.0.0.1:11434", "fixture").unwrap();
    let expected = preview(&target, "question", &sources()).unwrap();
    assert_eq!(expected.body()["think"], false);
    assert_eq!(expected.body()["runner"], "llamacpp");
    assert_eq!(expected.body()["truncate"], false);
    assert_eq!(expected.body()["shift"], false);
    assert_eq!(expected.body()["keep_alive"], 0);
    assert_eq!(expected.body()["options"]["num_ctx"], 8192);
    assert_eq!(expected.body()["options"]["num_predict"], 2048);
    let schema = &expected.body()["format"];
    assert_eq!(schema["oneOf"].as_array().unwrap().len(), 2);
    let insufficient = &schema["oneOf"][0];
    let complete = &schema["oneOf"][1];
    assert_eq!(insufficient["required"], json!(["decision"]));
    assert_eq!(
        insufficient["properties"]["decision"]["const"],
        "insufficient"
    );
    assert!(insufficient["properties"].get("excerpts").is_none());
    assert_eq!(complete["required"], json!(["decision", "excerpts"]));
    assert_eq!(complete["properties"]["decision"]["const"], "complete");
    assert_eq!(complete["properties"]["excerpts"]["minItems"], 1);
    assert_eq!(complete["properties"]["excerpts"]["maxItems"], 1);
    assert_eq!(complete["properties"]["excerpts"]["uniqueItems"], true);
    assert_eq!(
        complete["properties"]["excerpts"]["items"]["enum"],
        json!(["s1u1"])
    );
    for branch in schema["oneOf"].as_array().unwrap() {
        assert_eq!(branch["additionalProperties"], false);
        assert!(branch["properties"].get("response").is_none());
        assert!(branch["properties"].get("requirements").is_none());
    }
    let user: Value =
        serde_json::from_str(expected.body()["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(user["question"], "question");
    assert_eq!(user["evidence"], json!([{"id":1,"text":"唯一🙂证据"}]));
    assert_eq!(
        user["exact_excerpts"],
        json!([{"key":"s1u1","id":1,"quote":"唯一🙂证据"}])
    );
    let in_prompt = expected.body()["messages"][0]["content"]
        .as_str()
        .unwrap()
        .split("\nRequired JSON schema:\n")
        .nth(1)
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(in_prompt).unwrap(), *schema);
    assert_ne!(
        expected.fingerprint().unwrap(),
        preview(&target, "question ", &sources())
            .unwrap()
            .fingerprint()
            .unwrap()
    );
}
