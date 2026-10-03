use super::stream::TextStream;
#[test]
fn accepts_split_utf8_and_all_sse_line_endings() {
    for ending in ["\n", "\r\n", "\r"] {
        let data = format!(
            "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"你好\"}}{ending}{ending}data: {{\"type\":\"response.completed\",\"response\":{{\"status\":\"completed\"}}}}{ending}{ending}"
        );
        let mut stream = TextStream::default();
        let mut result = None;
        for byte in data.bytes() {
            if let Some(text) = stream.push(&[byte]).unwrap() {
                result = Some(text);
            }
        }
        assert_eq!(result.as_deref(), Some("你好"));
    }
}
#[test]
fn rejects_failure_after_partial_text_and_oversized_stream() {
    let mut stream = TextStream::default();
    assert!(
        stream
            .push(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n")
            .unwrap()
            .is_none()
    );
    let error = stream.push(b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_limit_exceeded\",\"message\":\"private\"}}}\n\n").unwrap_err();
    assert!(error.0.contains("limit"));
    assert!(!error.0.contains("private"));
    assert!(
        TextStream::default()
            .push(&vec![b'x'; 1024 * 1024 + 1])
            .is_err()
    );
    assert!(TextStream::default().push(b"data: [DONE]\n\n").is_err());
    assert!(
        TextStream::default()
            .push(b"data: {\"type\":\"response.completed\"}\n\n")
            .is_err()
    );
}

fn registration(scopes: &str) -> super::Registration {
    serde_json::from_value(serde_json::json!({"client_id":"oaiapp_test","subject":"user","email":null,"credentials":{"access_token":"test-token","refresh_token":"refresh-old","expires_at":super::now().unwrap()+3600,"scopes":scopes.split_whitespace().collect::<Vec<_>>()}})).unwrap()
}

#[tokio::test]
async fn transport_sends_subscription_contract_once_and_requires_completion() {
    use axum::{Json, Router, http::HeaderMap, routing::post};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let app = Router::new().route("/responses", post(move |headers: HeaderMap, Json(value): Json<serde_json::Value>| {
        let seen = seen.clone();
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            assert_eq!(headers["authorization"], "Bearer test-token");
            assert_eq!(value, serde_json::json!({"model":"available-model","input":[{"role":"user","content":"hello"}],"store":false,"stream":true}));
            ([("content-type", "text/event-stream")], "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n")
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = super::ChatGptClient::new().unwrap();
    client.resource = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let r = registration("resource.invoke chatgpt.tokens.use.direct");
    assert!(
        client
            .ask(&r, "available-model", "hello", false)
            .await
            .is_err()
    );
    assert!(
        client
            .ask(&registration("openid"), "available-model", "hello", true)
            .await
            .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let error = client
        .ask(&r, "available-model", "hello", true)
        .await
        .unwrap_err();
    assert!(error.0.contains("without completion"));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn refresh_rotates_tokens_and_scopes_and_does_not_retry_or_follow_redirects() {
    use axum::{Form, Json, Router, http::StatusCode, routing::post};
    use std::{
        collections::HashMap,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let app = Router::new().route("/api/accounts/oauth/token", post(move |Form(form): Form<HashMap<String, String>>| {
        let seen = seen.clone();
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            assert_eq!(form["client_id"], "oaiapp_test");
            assert_eq!(form["resource"], super::RESOURCE);
            assert!(!form.contains_key("scope"));
            Json(serde_json::json!({"access_token":"rotated","refresh_token":"rotated-refresh","scope":"openid","token_type":"Bearer","expires_in":3600}))
        }
    })).route("/responses", post(|| async { (StatusCode::TEMPORARY_REDIRECT, [("location", "/should-not-follow")]) }))
      .route("/should-not-follow", post(|| async { panic!("redirect followed"); #[allow(unreachable_code)] "" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = super::ChatGptClient::new().unwrap();
    client.auth = format!("http://{}", listener.local_addr().unwrap());
    client.resource = client.auth.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut r = registration("resource.invoke chatgpt.tokens.use.direct");
    assert!(
        client
            .ask(&r, "available-model", "hello", true)
            .await
            .is_err()
    );
    client.refresh(&mut r).await.unwrap();
    assert!(!r.plan_enabled());
    let value = serde_json::to_value(&r).unwrap();
    assert_eq!(value["credentials"]["refresh_token"], "rotated-refresh");
    // Discovery fails on the fixture; local logout must still discard every token.
    assert!(client.sign_out(&mut r).await.is_err());
    assert!(!r.signed_in());
    assert_eq!(r.client_id, "oaiapp_test");
    assert!(serde_json::to_value(&r).unwrap()["credentials"].is_null());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn catalog_uses_account_token_and_success_waits_for_completed_event() {
    use axum::{
        Json, Router,
        http::HeaderMap,
        routing::{get, post},
    };
    let app = Router::new().route("/models", get(|headers: HeaderMap| async move {
        assert_eq!(headers["authorization"], "Bearer test-token");
        Json(serde_json::json!({"models":[{"slug":"shown","display_name":"Shown","visibility":"list"},{"slug":"hidden","display_name":"Hidden","visibility":"hidden"}]}))
    })).route("/responses", post(|| async {
        ([("content-type", "text/event-stream; charset=utf-8")], "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n")
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = super::ChatGptClient::new().unwrap();
    client.resource = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let r = registration("resource.invoke chatgpt.tokens.use.direct");
    let models = client.models(&r).await.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].slug, "shown");
    assert_eq!(
        client.ask(&r, "shown", "hello", true).await.unwrap(),
        "hello"
    );
    server.abort();
}

#[tokio::test]
async fn scoring_maps_instructions_without_api_parameters_and_never_retries() {
    use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::post};
    use personal_ai_llm::{ChatMessage, ChatRequest, Role};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let app=Router::new().route("/responses",post(move |Json(value):Json<serde_json::Value>| {
        let seen=seen.clone(); async move {
            seen.fetch_add(1,Ordering::SeqCst);
            assert_eq!(value.as_object().unwrap().len(),5);
            assert_eq!(value["instructions"],"frozen system");
            assert_eq!(value["input"],serde_json::json!([{"role":"user","content":"x".repeat(40000)}]));
            assert_eq!(value["store"],false); assert_eq!(value["stream"],true);
            if value["model"]=="deny" { return (StatusCode::TOO_MANY_REQUESTS,"private quota error").into_response(); }
            ([("content-type","text/event-stream")],"data: {\"type\":\"response.output_text.delta\",\"delta\":\"{}\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n").into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = super::ChatGptClient::new().unwrap();
    client.resource = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let registration = registration("resource.invoke chatgpt.tokens.use.direct");
    let mut request = ChatRequest {
        messages: vec![
            ChatMessage {
                role: Role::System,
                content: "frozen system".into(),
            },
            ChatMessage {
                role: Role::User,
                content: "x".repeat(40000),
            },
        ],
        temperature: Some(0.0),
        max_output_tokens: Some(4096),
    };
    assert!(
        client
            .score_value(&registration, "model", &request, false)
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        client
            .score_value(&registration, "model", &request, true)
            .await
            .unwrap(),
        "{}"
    );
    let error = client
        .score_value(&registration, "deny", &request, true)
        .await
        .unwrap_err();
    assert!(!error.0.contains("private"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    request.messages[0].role = Role::User;
    assert!(
        client
            .score_value(&registration, "model", &request, true)
            .await
            .is_err()
    );
    request.messages[0].role = Role::System;
    request.messages[1].content = "x".repeat(65536);
    assert!(
        client
            .score_value(&registration, "model", &request, true)
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    server.abort();
}

#[test]
fn stream_errors_are_terminal_and_conflicting_completion_is_not_returned() {
    let delta = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"private\"}\n\n";
    let done =
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n";
    for tail in [done, delta, "data: {\"type\":\"error\"}\n\n"] {
        let mut stream = TextStream::default();
        assert!(
            stream
                .push(format!("{delta}{done}{tail}").as_bytes())
                .is_err()
        );
        assert!(stream.push(done.as_bytes()).is_err());
    }
    for invalid in ["data: broken\n\n", "data: {\"type\":\"error\"}\n\n"] {
        let mut stream = TextStream::default();
        stream.push(delta.as_bytes()).unwrap();
        assert!(stream.push(invalid.as_bytes()).is_err());
        assert!(stream.push(done.as_bytes()).is_err());
    }
}

#[tokio::test]
async fn transport_waits_for_eof_and_rejects_late_failure_truncation_and_timeout() {
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let done = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n";
    for mode in ["success", "late_error", "truncated", "timeout", "cancel"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sent, received) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut bytes = [0; 4096];
                let count = socket.read(&mut bytes).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&bytes[..count]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let tail = if mode == "late_error" {
                "data: {\"type\":\"error\"}\n\n"
            } else {
                ""
            };
            let length = done.len()
                + tail.len()
                + usize::from(matches!(mode, "truncated" | "timeout" | "cancel"));
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{done}").as_bytes()).await.unwrap();
            let _ = sent.send(());
            if matches!(mode, "timeout" | "cancel") {
                tokio::time::sleep(Duration::from_secs(5)).await;
            } else if !tail.is_empty() {
                tokio::time::sleep(Duration::from_millis(30)).await;
                socket.write_all(tail.as_bytes()).await.unwrap();
            }
        });
        let mut client = super::ChatGptClient::new().unwrap();
        client.resource = format!("http://{address}");
        client.client = reqwest::Client::builder()
            .no_proxy()
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        let request = tokio::spawn(async move {
            client
                .ask(
                    &registration("resource.invoke chatgpt.tokens.use.direct"),
                    "available-model",
                    "hello",
                    true,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), received)
            .await
            .unwrap()
            .unwrap();
        if mode == "cancel" {
            assert!(!request.is_finished());
            request.abort();
            assert!(request.await.unwrap_err().is_cancelled());
        } else {
            let result = request.await.unwrap();
            if mode == "success" {
                assert_eq!(result.unwrap(), "hello");
            } else {
                assert!(result.is_err(), "{mode}");
            }
        }
        server.abort();
    }
}

#[test]
fn provisional_notifications_preserve_split_unicode_and_never_include_provider_metadata() {
    use personal_ai_llm::stream::TextDeltaSink;
    #[derive(Default)]
    struct Sink(std::sync::Mutex<Vec<String>>);
    impl TextDeltaSink for Sink {
        fn delta(&self, text: &str) {
            self.0.lock().unwrap().push(text.into());
        }
    }
    let sink = Sink::default();
    let mut stream = TextStream::default();
    let events = "data: {\"type\":\"response.created\",\"private\":\"metadata\"}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"你好\"}\n\n";
    for byte in events.bytes() {
        assert!(stream.push_observed(&[byte], &sink).unwrap().is_none());
    }
    assert_eq!(*sink.0.lock().unwrap(), ["你好"]);
    assert!(
        stream
            .push_observed(b"data: {\"type\":\"error\"}\n\n", &sink)
            .is_err()
    );
    assert!(stream.push_observed(events.as_bytes(), &sink).is_err());
    assert_eq!(sink.0.lock().unwrap().len(), 1);
}
