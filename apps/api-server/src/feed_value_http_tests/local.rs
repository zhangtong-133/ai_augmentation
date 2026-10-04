use super::*;
use personal_ai_llm::{
    BoxFuture, ChatRequest, LlmResult,
    local::{LocalInference, LocalTarget},
    stream::TextDeltaSink,
};
use std::sync::atomic::{AtomicUsize, Ordering};
struct Runtime {
    calls: AtomicUsize,
    shared: serde_json::Value,
}
impl LocalInference for Runtime {
    fn infer<'a>(
        &'a self,
        _: &'a LocalTarget,
        request: &'a ChatRequest,
        _: &'a dyn TextDeltaSink,
    ) -> BoxFuture<'a, LlmResult<String>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.max_output_tokens, Some(2048));
            assert_eq!(self.shared["instructions"], request.messages[0].content);
            assert_eq!(self.shared["input"], request.messages[1].content);
            assert!(!request.messages[1].content.contains("private=secret"));
            Ok(r#"{"items":[{"id":1,"category":"high"}]}"#.into())
        })
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 和 RSS_LOCAL_ENABLED=true"]
#[allow(clippy::too_many_lines)]
async fn local_value_http_checks_exact_compute_consent_reading_and_source_erasure() {
    let (mut f, _) = setup().await;
    f.state.subscription_connections = None;
    let (_, config) = f.call("GET", "/api/feed-values/config", json!({})).await;
    assert_eq!(config["local_enabled"], true);
    let id = Uuid::new_v4().to_string();
    let path = format!("/api/feed-values/{id}");
    let input = json!({"id":id,"endpoint":"http://127.0.0.1:11435","model":"qwen3:4b-q4_K_M"});
    let (status, saved) = f
        .call("POST", "/api/feed-values/local", input.clone())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["pricing"]["kind"], "local");
    assert_eq!(saved["pricing"]["profile"], "local-rss-v4");
    for private in [
        "dispatch_token",
        "connection_id",
        "private=secret",
        "user_id",
    ] {
        assert!(!saved.to_string().contains(private));
    }
    assert_eq!(
        f.call("POST", "/api/feed-values/local", input).await.1,
        saved
    );
    let approval = json!({"digest":saved["digest"],"acknowledge_sharing":true,"acknowledge_local_compute":true});
    for (body, expected) in [
        (
            json!({"digest":saved["digest"],"acknowledge_sharing":true,"acknowledge_local_compute":false}),
            409,
        ),
        (
            json!({"digest":saved["digest"],"acknowledge_sharing":true,"acknowledge_local_compute":true,"acknowledge_subscription_usage":true}),
            422,
        ),
    ] {
        assert_eq!(
            f.call("POST", &format!("{path}/approve-local"), body)
                .await
                .0
                .as_u16(),
            expected
        );
    }
    assert_eq!(
        f.send(
            "POST",
            &format!("{path}/approve-local"),
            approval.clone(),
            Some(&f.other_cookie),
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.send(
            "POST",
            &format!("{path}/approve-local"),
            approval.clone(),
            Some(&f.cookie),
            false
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.call("POST", &format!("{path}/approve"), approve(&saved))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.call("POST", &format!("{path}/approve-local"), approval)
            .await
            .1["status"],
        "authorized"
    );
    let runtime = Runtime {
        calls: AtomicUsize::new(0),
        shared: saved["shared_content"].clone(),
    };
    let target = LocalTarget::new("http://127.0.0.1:11435", "qwen3:4b-q4_K_M").unwrap();
    let result = personal_ai_agent_core::feed_value_local::execute_local_value(
        f.store.as_ref(),
        &runtime,
        &target,
        &f.owner,
        &id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.status, "succeeded");
    assert_eq!(result.scores.as_ref().unwrap()[0].score, Some(80));
    assert!(
        personal_ai_agent_core::feed_value_local::execute_local_value(
            f.store.as_ref(),
            &runtime,
            &target,
            &f.owner,
            &id
        )
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.call("GET", &format!("{path}/reading"), json!({})).await.0,
        StatusCode::OK
    );
    let sub: String =
        sqlx::query_scalar("SELECT id::text FROM feed_subscriptions WHERE user_id=$1")
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    f.store
        .delete_subscription(&f.owner, &sub, 1)
        .await
        .unwrap();
    let (_, erased) = f.call("GET", &path, json!({})).await;
    assert_eq!(erased["status"], "invalidated");
    assert!(erased["scores"].is_null());
    assert!(erased["shared_content"].is_null());
    assert_eq!(
        f.call("GET", &format!("{path}/reading"), json!({})).await.0,
        StatusCode::CONFLICT
    );
    f.cleanup().await;
}
