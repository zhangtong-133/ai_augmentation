use super::*;
use axum::body::BodyDataStream;
use futures_util::StreamExt;

pub(super) async fn next(stream: &mut BodyDataStream) -> serde_json::Value {
    loop {
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(4), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        if let Some(data) = text.lines().find_map(|line| line.strip_prefix("data: ")) {
            return serde_json::from_str(data).unwrap();
        }
    }
}
pub(super) async fn open(f: &Fixture, path: &str) -> BodyDataStream {
    let response = f.send("GET", path, json!({}), Some(&f.cookie), false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    response.into_body().into_data_stream()
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_events_observe_separate_store_execution_without_dispatch_or_private_content() {
    let (f, _, request, proof) = authorized().await;
    let path = format!("/api/learning/model-authorizations/{request}/events");
    for (cookie, expected) in [(None, 401), (Some(f.other_cookie.as_str()), 404)] {
        assert_eq!(
            f.send("GET", &path, json!({}), cookie, false)
                .await
                .status()
                .as_u16(),
            expected
        );
    }
    assert_eq!(
        f.send(
            "GET",
            &format!("{path}?replay=1"),
            json!({}),
            Some(&f.cookie),
            false
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let mut stream = open(&f, &path).await;
    let first = next(&mut stream).await;
    assert_eq!(
        first,
        json!({"protocol_version":"learning-status-v1", "request_id":request, "sequence":"0", "detail":{"status":"authorized","terminal":false}})
    );
    let worker = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let claim = worker
        .claim_model_review(&f.owner, &request)
        .await
        .unwrap()
        .unwrap();
    let running = next(&mut stream).await;
    assert_eq!(running["detail"]["status"], "running");
    assert_eq!(running["sequence"], "1");
    assert!(worker.begin_model_review(&claim, &proof).await.unwrap());
    let finished = worker
        .finish_model_review(&claim, Some(output(&claim.authorization)))
        .await
        .unwrap();
    assert_eq!(finished.status, "succeeded");
    assert_eq!(
        next(&mut stream).await,
        json!({"protocol_version":"learning-status-v1", "request_id":request, "sequence":"2", "detail":{"status":"succeeded","terminal":true}})
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
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_events_limit_connections_release_on_drop_and_close_on_cancellation() {
    let (f, _, request, _) = authorized().await;
    let path = format!("/api/learning/model-authorizations/{request}/events");
    let first = open(&f, &path).await;
    let second = open(&f, &path).await;
    assert_eq!(
        f.send("GET", &path, json!({}), Some(&f.cookie), false)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(first);
    let mut third = open(&f, &path).await;
    assert_eq!(next(&mut third).await["detail"]["status"], "authorized");
    assert_eq!(
        f.call(
            "POST",
            &format!("/api/learning/model-authorizations/{request}/cancel"),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        next(&mut third).await["detail"],
        json!({"status":"cancelled","terminal":true})
    );
    assert!(third.next().await.is_none());
    drop(second);
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_events_recheck_session_before_each_status() {
    let (f, _, request, _) = authorized().await;
    let mut stream = open(
        &f,
        &format!("/api/learning/model-authorizations/{request}/events"),
    )
    .await;
    next(&mut stream).await;
    let response = f
        .send("POST", "/api/auth/logout", json!({}), Some(&f.cookie), true)
        .await;
    assert!(response.status().is_success());
    assert_eq!(
        next(&mut stream).await["detail"],
        json!({"reason":"session_unavailable"})
    );
    assert!(stream.next().await.is_none());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_events_invalidate_when_source_is_deleted() {
    let (f, evidence, request, _) = authorized().await;
    let mut stream = open(
        &f,
        &format!("/api/learning/model-authorizations/{request}/events"),
    )
    .await;
    next(&mut stream).await;
    let item = f
        .store
        .get_model_authorization(&f.owner, &request)
        .await
        .unwrap();
    let plan = f
        .call(
            "GET",
            &format!("/api/learning/plans/{}", item.plan_id),
            json!({}),
        )
        .await
        .1;
    let evidence_id = &plan["results"][0]["evidence"]["request_id"];
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
    assert_eq!(
        next(&mut stream).await["detail"],
        json!({"status":"invalidated","terminal":true})
    );
    assert!(stream.next().await.is_none());
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_events_end_after_bounded_observation_without_claiming() {
    let (f, _, request, _) = authorized().await;
    let start = tokio::time::Instant::now();
    let mut stream = open(
        &f,
        &format!("/api/learning/model-authorizations/{request}/events"),
    )
    .await;
    next(&mut stream).await;
    let closed = tokio::time::timeout(std::time::Duration::from_secs(23), async {
        loop {
            let bytes = stream.next().await.unwrap().unwrap();
            let text = std::str::from_utf8(&bytes).unwrap();
            if let Some(data) = text.lines().find_map(|line| line.strip_prefix("data: ")) {
                break serde_json::from_str::<serde_json::Value>(data).unwrap();
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(closed["detail"], json!({"reason":"observation_timeout"}));
    assert!(start.elapsed() >= std::time::Duration::from_secs(19));
    assert!(stream.next().await.is_none());
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
