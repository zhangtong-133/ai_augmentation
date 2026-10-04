use super::events::{next, open};
use super::*;
use futures_util::StreamExt;
use personal_ai_storage::learning::review_text::{ReviewTextBridge, TextKind, TextPacket};
use personal_ai_storage_redis::RedisReviewText;
async fn fixture() -> (
    Fixture,
    String,
    String,
    VerifiedSubscriptionConnection,
    RedisReviewText,
) {
    let (mut f, path, request, proof) = authorized().await;
    let bridge = RedisReviewText::new(
        &std::env::var("TEST_REDIS_URL").expect("disposable TEST_REDIS_URL required"),
    )
    .unwrap();
    f.state.learning_text = Some(Arc::new(bridge.clone()));
    (f, path, request, proof, bridge)
}
fn url(request: &str) -> String {
    format!("/api/learning/model-authorizations/{request}/text-events")
}
fn delta(sequence: u64, text: &str) -> TextPacket {
    TextPacket {
        sequence,
        detail: TextKind::Delta { text: text.into() },
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 和 TEST_REDIS_URL"]
#[allow(clippy::too_many_lines)]
async fn learning_text_events_observe_separate_execution_without_dispatch_or_replay() {
    let (f, _, request, proof, bridge) = fixture().await;
    for (cookie, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(f.other_cookie.as_str()), StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            f.send("GET", &url(&request), json!({}), cookie, false)
                .await
                .status(),
            expected
        );
    }
    for path in [
        format!("{}?after=0", url(&request)),
        url(&Uuid::nil().to_string()),
    ] {
        assert_eq!(
            f.send("GET", &path, json!({}), Some(&f.cookie), false)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let mut disabled = f.state.clone();
    disabled.learning_text = None;
    assert_eq!(
        auth_request(
            router(disabled),
            "GET",
            &url(&request),
            Some(&f.cookie),
            "{}",
            false
        )
        .await
        .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let mut stream = open(&f, &url(&request)).await;
    let first = next(&mut stream).await;
    assert_eq!(first["protocol_version"], "learning-text-v1");
    assert_eq!(first["detail"]["status"], "authorized");
    assert_eq!(
        f.store
            .get_model_authorization(&f.owner, &request)
            .await
            .unwrap()
            .status,
        "authorized"
    );
    let worker = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let claim = worker
        .claim_model_review(&f.owner, &request)
        .await
        .unwrap()
        .unwrap();
    assert!(worker.begin_model_review(&claim, &proof).await.unwrap());
    let mut publisher = bridge.publisher(&f.owner, &request).await.unwrap();
    publisher
        .publish(delta(0, "中文增量 <script>"))
        .await
        .unwrap();
    loop {
        let item = next(&mut stream).await;
        if item["detail"].get("text").is_some() {
            assert_eq!(item["detail"], json!({"text":"中文增量 <script>"}));
            assert_eq!(item.as_object().unwrap().len(), 4);
            break;
        }
    }
    // End cannot create a successful business state; only the durable store can.
    publisher
        .publish(TextPacket {
            sequence: 1,
            detail: TextKind::End,
        })
        .await
        .unwrap();
    loop {
        if next(&mut stream).await["detail"] == json!({}) {
            break;
        }
    }
    assert_eq!(
        worker
            .get_model_authorization(&f.owner, &request)
            .await
            .unwrap()
            .status,
        "running"
    );
    worker
        .finish_model_review(&claim, Some(output(&claim.authorization)))
        .await
        .unwrap();
    assert_eq!(
        next(&mut stream).await["detail"],
        json!({"status":"succeeded","terminal":true})
    );
    assert!(stream.next().await.is_none());
    assert!(
        worker
            .claim_model_review(&f.owner, &request)
            .await
            .unwrap()
            .is_none()
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 和 TEST_REDIS_URL"]
async fn learning_text_events_recheck_session_before_releasing_queued_body() {
    let (f, _, request, proof, bridge) = fixture().await;
    let mut stream = open(&f, &url(&request)).await;
    next(&mut stream).await;
    let claim = f
        .store
        .claim_model_review(&f.owner, &request)
        .await
        .unwrap()
        .unwrap();
    f.store.begin_model_review(&claim, &proof).await.unwrap();
    let mut publisher = bridge.publisher(&f.owner, &request).await.unwrap();
    publisher
        .publish(delta(0, "queued private text"))
        .await
        .unwrap();
    assert!(
        f.send("POST", "/api/auth/logout", json!({}), Some(&f.cookie), true)
            .await
            .status()
            .is_success()
    );
    assert_eq!(
        next(&mut stream).await["detail"],
        json!({"reason":"session_unavailable"})
    );
    assert!(stream.next().await.is_none());
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 和 TEST_REDIS_URL"]
async fn learning_text_events_discard_body_after_cancel_evidence_or_connection_revocation() {
    for mode in ["cancel", "evidence", "connection"] {
        let (f, evidence, request, proof, bridge) = fixture().await;
        let mut stream = open(&f, &url(&request)).await;
        next(&mut stream).await;
        let claim = f
            .store
            .claim_model_review(&f.owner, &request)
            .await
            .unwrap()
            .unwrap();
        f.store.begin_model_review(&claim, &proof).await.unwrap();
        let mut publisher = bridge.publisher(&f.owner, &request).await.unwrap();
        publisher
            .publish(delta(0, "must not escape after revocation"))
            .await
            .unwrap();
        match mode {
            "cancel" => {
                f.store
                    .cancel_model_authorization(&f.owner, &request)
                    .await
                    .unwrap();
            }
            "connection" => {
                f.store
                    .revoke_subscription_connection(&f.owner, &claim.authorization.connection_id, 1)
                    .await
                    .unwrap();
            }
            _ => {
                let plan = f
                    .store
                    .get_learning_plan(&f.owner, &claim.authorization.plan_id)
                    .await
                    .unwrap();
                let evidence_id = &plan.results[0].evidence.as_ref().unwrap().request_id;
                assert!(
                    f.send(
                        "DELETE",
                        &evidence,
                        json!({"request_id":evidence_id}),
                        Some(&f.cookie),
                        true
                    )
                    .await
                    .status()
                    .is_success()
                );
            }
        }
        let expected = if mode == "cancel" {
            "cancelled"
        } else {
            "invalidated"
        };
        assert_eq!(
            next(&mut stream).await["detail"],
            json!({"status":expected,"terminal":true})
        );
        assert!(stream.next().await.is_none());
        assert_ne!(
            f.store
                .finish_model_review(&claim, Some(output(&claim.authorization)))
                .await
                .unwrap()
                .status,
            "succeeded"
        );
        f.cleanup().await;
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 和 TEST_REDIS_URL"]
async fn learning_text_events_share_status_connection_limits_and_release_on_drop() {
    let (f, _, request, _, _) = fixture().await;
    let text = open(&f, &url(&request)).await;
    let status = open(
        &f,
        &format!("/api/learning/model-authorizations/{request}/events"),
    )
    .await;
    assert_eq!(
        f.send("GET", &url(&request), json!({}), Some(&f.cookie), false)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(text);
    let mut replacement = open(&f, &url(&request)).await;
    assert_eq!(
        next(&mut replacement).await["detail"]["status"],
        "authorized"
    );
    drop(status);
    drop(replacement);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 和 TEST_REDIS_URL"]
async fn learning_text_events_overflow_closes_observation_but_execution_can_still_save() {
    let (f, _, request, proof, bridge) = fixture().await;
    let mut stream = open(&f, &url(&request)).await;
    next(&mut stream).await;
    let claim = f
        .store
        .claim_model_review(&f.owner, &request)
        .await
        .unwrap()
        .unwrap();
    f.store.begin_model_review(&claim, &proof).await.unwrap();
    let mut publisher = bridge.publisher(&f.owner, &request).await.unwrap();
    for sequence in 0..40 {
        publisher.publish(delta(sequence, "private")).await.unwrap();
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    loop {
        let item = next(&mut stream).await;
        assert!(item["detail"].get("text").is_none());
        if item["detail"].get("reason").is_some() {
            assert_eq!(item["detail"], json!({"reason":"observation_unavailable"}));
            break;
        }
    }
    assert!(stream.next().await.is_none());
    assert_eq!(
        f.store
            .finish_model_review(&claim, Some(output(&claim.authorization)))
            .await
            .unwrap()
            .status,
        "succeeded"
    );
    assert!(
        f.store
            .claim_model_review(&f.owner, &request)
            .await
            .unwrap()
            .is_none()
    );
    f.cleanup().await;
}
