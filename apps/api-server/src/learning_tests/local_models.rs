use super::{
    reviews::{evidence, setup},
    *,
};
use personal_ai_agent_core::learning_model_execution::local::observe_local_review;
use personal_ai_llm::local::LocalTarget;
use personal_ai_storage::{
    learning::{
        LearningStore,
        model_authorization::{LocalReviewExecutionStore, ModelReviewExecutionStore},
    },
    learning_operations::LearningOperationsStore,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
fn target() -> LocalTarget {
    LocalTarget::new("http://127.0.0.1:11435", "qwen3:4b").unwrap()
}
async fn draft(f: &Fixture, path: &str, request: &str, target: &LocalTarget) -> serde_json::Value {
    let (status, value) = f
        .call(
            "POST",
            &format!("{path}/local-model-authorizations"),
            json!({"request_id":request,"endpoint":target.endpoint(),"model":target.model()}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    value
}
async fn authorize(f: &Fixture, value: &serde_json::Value) {
    assert_eq!(f.call("POST", &format!("/api/learning/model-authorizations/{}/approve-local", value["request_id"].as_str().unwrap()), json!({"digest":value["digest"],"acknowledge_sharing":true,"acknowledge_local_compute":true})).await.0, StatusCode::OK);
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 和 LEARNING_LOCAL_ENABLED=true"]
async fn learning_local_consent_is_private_exact_idempotent_and_separate_from_subscription() {
    let (f, _, path, _) = setup().await;
    evidence(&f, &path).await;
    let request = Uuid::new_v4().to_string();
    let input =
        json!({"request_id":request,"endpoint":target().endpoint(),"model":target().model()});
    let url = format!("{path}/local-model-authorizations");
    assert_eq!(
        f.send("POST", &url, input.clone(), None, true)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.send("POST", &url, input.clone(), Some(&f.cookie), false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.send("POST", &url, input.clone(), Some(&f.other_cookie), true)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    for endpoint in ["http://192.168.1.1:11435", "http://127.0.0.1:11435/path"] {
        let mut bad = input.clone();
        bad["endpoint"] = json!(endpoint);
        assert_eq!(f.call("POST", &url, bad).await.0, StatusCode::BAD_REQUEST);
    }
    let item = draft(&f, &path, &request, &target()).await;
    assert_eq!(item["local_endpoint"], target().endpoint());
    assert_eq!(item["connection_id"], "");
    assert_eq!(draft(&f, &path, &request, &target()).await, item);
    let mut changed = input.clone();
    changed["model"] = json!("other");
    assert_eq!(f.call("POST", &url, changed).await.0, StatusCode::CONFLICT);
    let root = format!("/api/learning/model-authorizations/{request}");
    assert_eq!(f.call("POST", &format!("{root}/approve"), json!({"digest":item["digest"],"acknowledge_sharing":true,"acknowledge_subscription_usage":true})).await.0, StatusCode::CONFLICT);
    assert_eq!(f.call("POST", &format!("{root}/approve-local"), json!({"digest":item["digest"],"acknowledge_sharing":true,"acknowledge_local_compute":false})).await.0, StatusCode::BAD_REQUEST);
    authorize(&f, &item).await;
    assert!(
        f.store
            .claim_model_review(&f.owner, &request)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.store
            .get_model_authorization(&f.owner, &request)
            .await
            .unwrap()
            .status,
        "authorized"
    );
    f.cleanup().await;
}
fn output(item: &personal_ai_storage::learning::model_authorization::ModelAuthorization) -> String {
    let preview = item.preview.as_ref().unwrap();
    let mut out = json!({"protocol_version":"learning-model-review-v1","input_digest":preview.input().input_digest});
    for field in ["explanation", "work", "verification", "limitations"] {
        out[field] = json!({"verdict":"unverified","reason":"本地建议仍需人工核验","citations":[]});
    }
    out.to_string()
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 和 LEARNING_LOCAL_ENABLED=true"]
async fn learning_local_oversized_material_is_rejected_without_draft_or_truncation() {
    let (f, _, path, _) = setup().await;
    assert_eq!(f.call("POST", &path, json!({"request_id":Uuid::new_v4(),"body":{
        "explanation":"例".repeat(1500),"work":"独立产物","verification":"验证过程","limitations":"局限"}})).await.0, StatusCode::OK);
    let request = Uuid::new_v4().to_string();
    assert_eq!(
        f.call(
            "POST",
            &format!("{path}/local-model-authorizations"),
            json!({"request_id":request,"endpoint":target().endpoint(),"model":target().model()})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.call(
            "GET",
            &format!("/api/learning/model-authorizations/{request}"),
            json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 和 LEARNING_LOCAL_ENABLED=true"]
async fn learning_local_http_execution_sends_once_saves_strict_advice_and_audits_without_subscription()
 {
    let (f, _, path, _) = setup().await;
    evidence(&f, &path).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = LocalTarget::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        "qwen3:4b",
    )
    .unwrap();
    let request = Uuid::new_v4().to_string();
    let item = draft(&f, &path, &request, &target).await;
    authorize(&f, &item).await;
    let saved = f
        .store
        .get_model_authorization(&f.owner, &request)
        .await
        .unwrap();
    let output = output(&saved);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let digest = saved.preview.as_ref().unwrap().input().input_digest.clone();
    let app = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
        counter.fetch_add(1, Ordering::SeqCst); assert_eq!(body["model"], "qwen3:4b"); assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
        let shared:serde_json::Value=serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap(); assert_eq!(shared["input_digest"], digest);
        let wire = format!("data: {}\n\ndata: [DONE]\n\n", json!({"model":"qwen3:4b","choices":[{"index":0,"delta":{"role":"assistant","content":output},"finish_reason":"stop"}]}));
        async move { ([("content-type","text/event-stream")], wire) }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let runtime = personal_ai_llm_local::LocalChatClient::new().unwrap();
    let (_, a) = observe_local_review(f.store.as_ref(), &runtime, &target, &f.owner, &request);
    let (_, b) = observe_local_review(f.store.as_ref(), &runtime, &target, &f.owner, &request);
    let (a, b) = tokio::join!(a, b);
    assert_eq!(
        usize::from(a.unwrap().is_some()) + usize::from(b.unwrap().is_some()),
        1
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let saved = f
        .store
        .get_model_authorization(&f.owner, &request)
        .await
        .unwrap();
    assert_eq!(saved.status, "succeeded");
    assert!(saved.advice.is_some());
    let audit = f.store.audit_learning_models(&f.owner, None).await.unwrap();
    assert!(audit.consistent);
    assert_eq!(audit.items[0].execution_kind, "local");
    assert!(
        f.store
            .claim_local_review(&f.owner, &request)
            .await
            .unwrap()
            .is_none()
    );
    server.abort();
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 和 LEARNING_LOCAL_ENABLED=true"]
async fn learning_local_dispatch_rechecks_target_cancel_and_source_and_never_reclaims() {
    for action in ["target", "cancel", "source", "invalid"] {
        let (f, _, path, _) = setup().await;
        let evidence_id = evidence(&f, &path).await;
        let request = Uuid::new_v4().to_string();
        let item = draft(&f, &path, &request, &target()).await;
        authorize(&f, &item).await;
        let claim = f
            .store
            .claim_local_review(&f.owner, &request)
            .await
            .unwrap()
            .unwrap();
        if action == "target" {
            let other = LocalTarget::new("http://127.0.0.1:11436", "qwen3:4b").unwrap();
            assert!(f.store.begin_local_review(&claim, &other).await.is_err());
            assert!(
                f.store
                    .finish_model_review(&claim, Some(output(&claim.authorization).into_bytes()))
                    .await
                    .unwrap()
                    .advice
                    .is_none()
            );
        } else {
            assert!(f.store.begin_local_review(&claim, &target()).await.unwrap());
            assert!(!f.store.begin_local_review(&claim, &target()).await.unwrap());
            if action == "cancel" {
                f.call(
                    "POST",
                    &format!("/api/learning/model-authorizations/{request}/cancel"),
                    json!({}),
                )
                .await;
            }
            if action == "source" {
                f.call("DELETE", &path, json!({"request_id":evidence_id}))
                    .await;
            }
            let bytes = if action == "invalid" {
                b"{}".to_vec()
            } else {
                output(&claim.authorization).into_bytes()
            };
            let saved = f
                .store
                .finish_model_review(&claim, Some(bytes))
                .await
                .unwrap();
            assert!(saved.advice.is_none());
            assert_ne!(saved.status, "succeeded");
        }
        assert!(
            f.store
                .claim_local_review(&f.owner, &request)
                .await
                .unwrap()
                .is_none()
        );
        f.cleanup().await;
    }
}
