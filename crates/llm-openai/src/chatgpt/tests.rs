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
