use super::*;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn periodic_feed_http_requires_exact_consent_and_keeps_history_private() {
    let f = Fixture::new(true).await;
    let sub = f.sub().await;
    let preview_path = format!(
        "/api/feed-subscriptions/{}/schedules",
        sub["snapshot"]["subscription_id"].as_str().unwrap()
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let body = json!({"schedule_id":Uuid::new_v4(), "starts_at_unix_ms":(now+600_000).to_string(), "ends_at_unix_ms":(now+7_800_000).to_string(), "interval_hours":1});
    assert_preview_guards(&f, &preview_path, &body).await;
    let response = f
        .send("POST", &preview_path, body.clone(), Some(&f.cookie), true)
        .await;
    assert_eq!(response.headers()["cache-control"], "no-store");
    let (status, draft) = decode(response).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        draft["plan"]["input"]["starts_at_unix_ms"],
        body["starts_at_unix_ms"]
    );
    assert!(draft["plan"]["created_at_unix_ms"].is_string());
    assert_eq!(f.call("POST", &preview_path, body).await.1, draft);
    let path = format!(
        "/api/feed-schedules/{}",
        draft["plan"]["input"]["schedule_id"].as_str().unwrap()
    );
    let consent =
        json!({"accepted_digest":draft["digest"], "acknowledge_recurring_source_requests":true});
    assert_private_schedule(&f, &path, &consent, &draft).await;
    let mut rejected = consent.clone();
    rejected["acknowledge_recurring_source_requests"] = json!(false);
    assert_eq!(
        f.call("POST", &format!("{path}/approve"), rejected).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut wrong = consent.clone();
    wrong["accepted_digest"] = json!("0".repeat(64));
    assert_eq!(
        f.call("POST", &format!("{path}/approve"), wrong).await.0,
        StatusCode::CONFLICT
    );
    let (status, active) = f
        .call("POST", &format!("{path}/approve"), consent.clone())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(active["status"], "active");
    assert!(active["approved_at_unix_ms"].is_string());
    assert_eq!(
        f.call("POST", &format!("{path}/approve"), consent.clone())
            .await
            .1,
        active
    );
    assert_eq!(
        f.call(
            "POST",
            &format!("{path}/cancel"),
            json!({"unexpected":true})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let cancelled = f.call("POST", &format!("{path}/cancel"), json!({})).await.1;
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(
        f.call("POST", &format!("{path}/cancel"), json!({})).await.1,
        cancelled
    );
    assert_eq!(
        f.call("POST", &format!("{path}/approve"), consent).await.0,
        StatusCode::CONFLICT
    );
    let events = f.call("GET", &format!("{path}/audit"), json!({})).await.1;
    assert_eq!(events.as_array().unwrap().len(), 3);
    assert!(events[0]["at_unix_ms"].is_string());
    assert_eq!(f.transport.calls.load(Ordering::SeqCst), 0);
    f.cleanup().await;
}

async fn assert_preview_guards(f: &Fixture, preview_path: &str, body: &serde_json::Value) {
    assert_eq!(
        f.send("POST", preview_path, body.clone(), None, true)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.send("POST", preview_path, body.clone(), Some(&f.cookie), false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for (field, value, expected) in [
        (
            "starts_at_unix_ms",
            json!(123),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        ("starts_at_unix_ms", json!("01"), StatusCode::BAD_REQUEST),
        (
            "ends_at_unix_ms",
            json!("9223372036854775808"),
            StatusCode::BAD_REQUEST,
        ),
        ("interval_hours", json!(2), StatusCode::BAD_REQUEST),
        (
            "owner",
            json!(f.other.as_str()),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let mut invalid = body.clone();
        invalid[field] = value;
        assert_eq!(f.call("POST", preview_path, invalid).await.0, expected);
    }
}

async fn assert_private_schedule(
    f: &Fixture,
    path: &str,
    consent: &serde_json::Value,
    draft: &serde_json::Value,
) {
    for (method, route, data) in [
        ("GET", path.to_owned(), json!({})),
        ("GET", format!("{path}/audit"), json!({})),
        ("POST", format!("{path}/approve"), consent.clone()),
        ("POST", format!("{path}/cancel"), json!({})),
    ] {
        let response = f
            .send(method, &route, data.clone(), Some(&f.other_cookie), true)
            .await;
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            f.send(method, &route, data.clone(), None, true)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        if method == "POST" {
            assert_eq!(
                f.send(method, &route, data, Some(&f.cookie), false)
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
    }
    assert_eq!(
        decode(
            f.send(
                "GET",
                "/api/feed-schedules",
                json!({}),
                Some(&f.other_cookie),
                false
            )
            .await
        )
        .await
        .1["items"],
        json!([])
    );
    assert_eq!(
        f.call("GET", "/api/feed-schedules?after=invalid", json!({}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.call("GET", "/api/feed-schedules?owner=other", json!({}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.call("GET", "/api/feed-schedules", json!({})).await.1["items"],
        json!([draft])
    );
}
