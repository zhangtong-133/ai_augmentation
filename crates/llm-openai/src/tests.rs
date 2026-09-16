use super::*;
#[test]
fn response_order_and_shape_are_validated() {
    let response =
        br#"{"model":"m","data":[{"index":1,"embedding":[0,1]},{"index":0,"embedding":[1,0]}]}"#;
    let values = decode(response, 2, "m", 2).unwrap();
    assert_eq!(values[0].values, vec![1.0, 0.0]);
    for response in [
        r#"{"model":"m","data":[{"index":0,"embedding":[1,0]},{"index":0,"embedding":[0,1]}]}"#,
        r#"{"model":"other","data":[{"index":0,"embedding":[1,0]}]}"#,
        r#"{"model":"m","data":[{"index":0,"embedding":[0,0]}]}"#,
    ] {
        assert!(decode(response.as_bytes(), 2, "m", 2).is_err());
    }
    assert!(decode(response, 2, "m", 3).is_err());
}

async fn server(
    status: reqwest::StatusCode,
    body: String,
) -> (String, tokio::task::JoinHandle<()>) {
    use axum::{Json, Router, http::HeaderMap, routing::post};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = Router::new().route(
        "/v1/embeddings",
        post(
            move |headers: HeaderMap, Json(input): Json<serde_json::Value>| {
                let body = body.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer test-key");
                    assert_eq!(input["model"], "m");
                    assert_eq!(input["dimensions"], 2);
                    assert_eq!(input["encoding_format"], "float");
                    (status, body)
                }
            },
        ),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, task)
}
fn local_provider(base: &str) -> OpenAiEmbeddings {
    let mut provider = OpenAiEmbeddings::new(base, "test-key", "m", 2).unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_static("Bearer test-key"),
    );
    provider.client = Client::builder()
        .no_proxy()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    provider
}
#[tokio::test]
async fn http_contract_reorders_vectors_and_sanitizes_provider_failures() {
    let (base, task) = server(
        reqwest::StatusCode::OK,
        r#"{"model":"m","data":[{"index":1,"embedding":[0,1]},{"index":0,"embedding":[1,0]}]}"#
            .into(),
    )
    .await;
    let provider = local_provider(&base);
    assert_eq!(
        provider
            .embedding(&["甲".into(), "乙".into()])
            .await
            .unwrap()[0]
            .values,
        vec![1.0, 0.0]
    );
    assert!(provider.embedding(&[]).await.is_err());
    task.abort();
    for status in [
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        reqwest::StatusCode::UNAUTHORIZED,
        reqwest::StatusCode::TEMPORARY_REDIRECT,
    ] {
        let (base, task) = server(status, "secret provider response".into()).await;
        let error = local_provider(&base)
            .embedding(&["甲".into()])
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("secret"));
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(error, LlmError::RateLimited);
        }
        task.abort();
    }
}
