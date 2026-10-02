use super::*;
use personal_ai_storage::schedules::{
    NewSchedule, ScheduleApproval, ScheduleDeliveryStore, ScheduleStore,
};

fn input() -> serde_json::Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    json!({"request_id":Uuid::new_v4().to_string(),"title":"提醒","body":"私有正文","run_at_unix_ms":(now+3_600_000).to_string()})
}
fn approval(s: &serde_json::Value) -> serde_json::Value {
    json!({"digest":s["digest"],"accepted_run_at_unix_ms":s["run_at_unix_ms"],"accepted_max_runs":1,"accepted_amount_micro":"0","acknowledge_schedule":true})
}
async fn call(
    f: &Fixture,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    f.call(method, path, body, Some(&f.cookie), true).await
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn schedule_http_requires_identity_csrf_and_exact_string_consent() {
    let f = Fixture::new().await;
    let input = input();
    assert_eq!(
        f.call("POST", "/api/schedules", input.clone(), None, true)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.call(
            "POST",
            "/api/schedules",
            input.clone(),
            Some(&f.cookie),
            false
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for invalid in [
        json!({"run_at_unix_ms":123}),
        json!({"owner":"forged"}),
        json!({"run_at_unix_ms":"00123"}),
    ] {
        let mut bad = input.clone();
        bad.as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        assert!(
            call(&f, "POST", "/api/schedules", bad)
                .await
                .0
                .is_client_error()
        );
    }
    let (code, draft) = call(&f, "POST", "/api/schedules", input.clone()).await;
    assert_eq!(code, StatusCode::CREATED);
    assert!(draft["run_at_unix_ms"].is_string());
    assert!(draft["created_at_unix_ms"].is_string());
    assert_eq!(draft["amount_micro"], "0");
    assert!(draft.get("claim_id").is_none());
    assert_eq!(call(&f, "POST", "/api/schedules", input).await.1, draft);
    let path = format!("/api/schedules/{}", draft["request_id"].as_str().unwrap());
    for invalid in [
        json!({"accepted_amount_micro":0}),
        json!({"accepted_amount_micro":"00"}),
        json!({"accepted_max_runs":2}),
        json!({"acknowledge_schedule":false}),
        json!({"claim_id":Uuid::new_v4()}),
    ] {
        let mut bad = approval(&draft);
        bad.as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        assert!(
            call(&f, "POST", &format!("{path}/approve"), bad)
                .await
                .0
                .is_client_error()
        );
    }
    let approved = call(&f, "POST", &format!("{path}/approve"), approval(&draft)).await;
    assert_eq!(approved.0, StatusCode::OK);
    assert_eq!(approved.1["status"], "scheduled");
    assert_eq!(
        call(&f, "POST", &format!("{path}/approve"), approval(&draft))
            .await
            .1,
        approved.1
    );
    assert_eq!(
        call(&f, "POST", &format!("{path}/cancel"), json!({}))
            .await
            .1["status"],
        "cancelled"
    );
    assert_eq!(
        call(&f, "POST", &format!("{path}/approve"), approval(&draft))
            .await
            .0,
        StatusCode::CONFLICT
    );
    for route in ["/api/schedules?after=invalid", "/api/reminders?user=forged"] {
        assert_eq!(
            call(&f, "GET", route, json!({})).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn schedule_http_isolates_delivery_and_does_not_expose_worker_credentials() {
    let f = Fixture::new().await;
    let other = Fixture::new().await;
    let input: serde_json::Value = input();
    let draft = call(&f, "POST", "/api/schedules", input).await.1;
    let request = draft["request_id"].as_str().unwrap();
    let path = format!("/api/schedules/{request}");
    for (method, suffix, body) in [
        ("GET", "", json!({})),
        ("POST", "/approve", approval(&draft)),
        ("POST", "/cancel", json!({})),
    ] {
        assert_eq!(
            call(&other, method, &format!("{path}{suffix}"), body)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        call(&f, "POST", &format!("{path}/approve"), approval(&draft))
            .await
            .0,
        StatusCode::OK
    );
    // 将完整授权夹具回拨到过去，以真实领取/投递验证查询，不引入测试 HTTP 后门。
    let now: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let stored = f.store.get_schedule(&f.user.id, request).await.unwrap();
    let input = NewSchedule {
        request_id: request.into(),
        title: stored.title,
        body: stored.body,
        run_at_unix_ms: now - 1000,
    };
    let digest = personal_ai_agent_core::schedules::schedule_digest(&f.user.id, &input);
    let consent = ScheduleApproval {
        digest: digest.clone(),
        accepted_run_at_unix_ms: input.run_at_unix_ms,
        accepted_max_runs: 1,
        accepted_amount_micro: 0,
        acknowledge_schedule: true,
    };
    sqlx::query("UPDATE schedules SET run_at_ms=$3,digest=$4,approval=$5,created_ms=$3-3600000,approval_expires_ms=$3-1800000,approved_ms=$3-3500000 WHERE user_id=$1 AND request_id=$2")
        .bind(Uuid::parse_str(f.user.id.as_str()).unwrap()).bind(Uuid::parse_str(request).unwrap()).bind(input.run_at_unix_ms).bind(digest).bind(serde_json::to_value(consent).unwrap()).execute(&f.pool).await.unwrap();
    let lease = f.store.claim_due_schedule().await.unwrap().unwrap();
    f.store.deliver_schedule(&lease).await.unwrap();
    assert_eq!(
        call(&f, "POST", &format!("{path}/cancel"), json!({}))
            .await
            .1["status"],
        "delivered"
    );
    let reminders = call(&f, "GET", "/api/reminders", json!({})).await.1;
    assert_eq!(reminders["items"][0]["body"], "私有正文");
    assert!(reminders["items"][0]["delivered_at_unix_ms"].is_string());
    assert_eq!(
        call(&other, "GET", "/api/reminders", json!({})).await.1["items"],
        json!([])
    );
    assert_eq!(
        call(&f, "POST", &format!("{path}/claim"), json!({}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let response = router(f.state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/reminders")
                .header("cookie", &f.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    check_reminder_inbox(&f, &other, request).await;
    other.cleanup().await;
    f.cleanup().await;
}

async fn check_reminder_inbox(f: &Fixture, other: &Fixture, request: &str) {
    let path = format!("/api/reminders/{request}");
    let body = json!({"revision":"0","read":true,"archived":true});
    assert_eq!(
        f.call("PUT", &path, body.clone(), None, true).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.call("PUT", &path, body.clone(), Some(&f.cookie), false)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(other, "PUT", &path, body.clone()).await.0,
        StatusCode::NOT_FOUND
    );
    for invalid in [
        json!({"revision":0}),
        json!({"revision":"00"}),
        json!({"revision":"-1"}),
        json!({"owner":"forged"}),
        json!({"claim_id":"forged"}),
    ] {
        let mut bad = body.clone();
        bad.as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        assert!(call(f, "PUT", &path, bad).await.0.is_client_error());
    }
    let (status, saved) = call(f, "PUT", &path, body.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["revision"], "1");
    assert!(saved["read_at_unix_ms"].is_string());
    assert!(saved["archived_at_unix_ms"].is_string());
    assert_eq!(call(f, "PUT", &path, body).await.1, saved);
    assert_eq!(
        call(f, "GET", "/api/reminders", json!({})).await.1["items"],
        json!([])
    );
    assert_eq!(
        call(f, "GET", "/api/reminders?archived=true", json!({}))
            .await
            .1["items"],
        json!([saved])
    );
    assert_eq!(
        call(f, "GET", "/api/reminders?archived=invalid", json!({}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            f,
            "PUT",
            &path,
            json!({"revision":"0","read":false,"archived":false})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            f,
            "PUT",
            &path,
            json!({"revision":"1","read":false,"archived":false})
        )
        .await
        .0,
        StatusCode::OK
    );
    let restored = call(f, "GET", "/api/reminders", json!({})).await.1;
    assert_eq!(restored["items"][0]["revision"], "2");
    assert!(restored["items"][0]["read_at_unix_ms"].is_null());
    assert!(restored["items"][0]["archived_at_unix_ms"].is_null());
}
