use super::*;
use personal_ai_storage::{
    briefs::BriefStore,
    feed_value::FeedValueExecutionStore,
    feeds::{CollectionOutcome, FeedStore, SubscriptionInput},
};
use sqlx::Row;
async fn setup() -> (Fixture, String) {
    let f = Fixture::new().await;
    let connection = f.connection().await;
    let sub = f
        .store
        .create_subscription(
            &f.owner,
            &Uuid::new_v4().to_string(),
            &SubscriptionInput {
                name: "private source".into(),
                source_url: "https://example.com/rss?private=secret".into(),
                enabled: true,
            },
        )
        .await
        .unwrap();
    let preview = f
        .store
        .preview_collection(
            &f.owner,
            &sub.snapshot.subscription_id,
            &Uuid::new_v4().to_string(),
        )
        .await
        .unwrap();
    let claim = f
        .store
        .claim_collection(&f.owner, &preview.plan.request_id, &preview.digest)
        .await
        .unwrap();
    f.store.finish_collection(&claim,CollectionOutcome::Response(b"<rss version=\"2.0\"><channel><title>News</title><description>News</description><link>https://example.com/</link><item><guid>one</guid><title>Rust &lt;script&gt;title&lt;/script&gt;</title><description>Rust summary</description></item></channel></rss>".to_vec())).await.unwrap();
    f.store
        .save_brief_preferences(&f.owner, 0, &["Rust".into()])
        .await
        .unwrap();
    (f, connection)
}
fn preview(connection: &str) -> serde_json::Value {
    json!({"id":Uuid::new_v4().to_string(),"connection_id":connection,"connection_revision":"1","model":"fixture"})
}
fn approve(saved: &serde_json::Value) -> serde_json::Value {
    json!({"digest":saved["digest"],"acknowledge_sharing":true,"acknowledge_subscription_usage":true})
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn value_http_requires_private_exact_consent_and_reads_completed_results() {
    let (f, connection) = setup().await;
    let input = preview(&connection);
    let path = format!("/api/feed-values/{}", input["id"].as_str().unwrap());
    let (status, saved) = f.call("POST", "/api/feed-values", input.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["status"], "draft");
    assert_eq!(saved["execution_mode"], "local_only");
    assert!(saved["created_at_unix_ms"].is_string());
    assert!(saved.get("snapshot").is_none());
    for private in [
        "private-subject",
        "oaiapp_",
        "private=secret",
        "dispatch_token",
        "host_id",
        "user_id",
    ] {
        assert!(!saved.to_string().contains(private));
    }
    assert_eq!(f.call("POST", "/api/feed-values", input).await.1, saved);
    let (_, page) = f.call("GET", "/api/feed-values", json!({})).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert!(page["items"][0].get("shared_content").is_none());
    assert!(page["items"][0].get("scores").is_none());
    for (method, url, body) in [
        ("GET", path.clone(), json!({})),
        ("GET", format!("{path}/audit"), json!({})),
        ("POST", format!("{path}/approve"), approve(&saved)),
        ("POST", format!("{path}/cancel"), json!({})),
    ] {
        let response = f
            .send(method, &url, body, Some(&f.other_cookie), true)
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let mut wrong = approve(&saved);
    wrong["acknowledge_subscription_usage"] = json!(false);
    assert_eq!(
        f.call("POST", &format!("{path}/approve"), wrong).await.0,
        StatusCode::CONFLICT
    );
    let mut wrong = approve(&saved);
    wrong["digest"] = json!("b".repeat(64));
    assert_eq!(
        f.call("POST", &format!("{path}/approve"), wrong).await.0,
        StatusCode::CONFLICT
    );
    let (_, authorized) = f
        .call("POST", &format!("{path}/approve"), approve(&saved))
        .await;
    assert_eq!(authorized["status"], "authorized");
    assert_eq!(
        f.call("POST", &format!("{path}/approve"), approve(&saved))
            .await
            .1,
        authorized
    );
    assert_eq!(
        f.call("POST", &format!("{path}/run"), json!({})).await.0,
        StatusCode::NOT_FOUND
    );
    let key = saved["id"].as_str().unwrap();
    let claim = f
        .store
        .claim_subscription_value(&f.owner, key)
        .await
        .unwrap()
        .unwrap();
    let row=sqlx::query("SELECT host_id,client_id,expires_ms FROM subscription_connections WHERE user_id=$1 AND id=$2").bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&connection).unwrap()).fetch_one(&f.pool).await.unwrap();
    let proof = VerifiedSubscriptionConnection {
        host_id: row.get::<Uuid, _>("host_id").urn().to_string(),
        client_id: row.get("client_id"),
        subject: "private-subject".into(),
        label: "personal".into(),
        models: vec!["fixture".into()],
        valid_until_unix_ms: row.get("expires_ms"),
    };
    assert!(
        f.store
            .begin_subscription_value(&claim, &proof)
            .await
            .unwrap()
    );
    f.store
        .finish_subscription_value(
            &claim,
            Some(br#"{"items":[{"id":1,"score":null,"reason":"insufficient evidence"}]}"#.to_vec()),
        )
        .await
        .unwrap();
    let (_, complete) = f.call("GET", &path, json!({})).await;
    assert_eq!(complete["status"], "succeeded");
    assert!(complete["scores"][0]["score"].is_null());
    assert_eq!(complete["candidates"][0]["id"], 1);
    let (_, audit) = f.call("GET", &format!("{path}/audit"), json!({})).await;
    assert!(
        audit["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["at_unix_ms"].is_string())
    );
    assert_eq!(
        audit["items"].as_array().unwrap().last().unwrap()["event"],
        "succeeded"
    );
    f.store
        .revoke_subscription_connection(&f.owner, &connection, 1)
        .await
        .unwrap();
    let (_, cleared) = f.call("GET", &path, json!({})).await;
    assert_eq!(cleared["status"], "invalidated");
    assert!(
        cleared["shared_content"].is_null()
            && cleared["candidates"].is_null()
            && cleared["scores"].is_null()
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn value_http_rejects_client_prices_identity_and_stale_connections() {
    let (f, connection) = setup().await;
    for field in [
        "user_id",
        "amount",
        "currency",
        "provider",
        "snapshot",
        "access_token",
        "acknowledge_cost",
    ] {
        let mut input = preview(&connection);
        input[field] = json!("untrusted");
        assert_eq!(
            f.call("POST", "/api/feed-values", input).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let mut input = preview(&connection);
    input["connection_revision"] = json!(1);
    assert_eq!(
        f.call("POST", "/api/feed-values", input).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    for revision in ["0", "01", "1001"] {
        let mut input = preview(&connection);
        input["connection_revision"] = json!(revision);
        assert_eq!(
            f.call("POST", "/api/feed-values", input).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut input = preview(&connection);
    input["connection_revision"] = json!("2");
    assert_eq!(
        f.call("POST", "/api/feed-values", input).await.0,
        StatusCode::CONFLICT
    );
    let mut input = preview(&connection);
    input["model"] = json!("other");
    assert_eq!(
        f.call("POST", "/api/feed-values", input).await.0,
        StatusCode::CONFLICT
    );
    let response = f
        .send(
            "POST",
            "/api/feed-values",
            preview(&connection),
            Some(&f.other_cookie),
            true,
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let (_, saved) = f
        .call("POST", "/api/feed-values", preview(&connection))
        .await;
    let path = format!("/api/feed-values/{}", saved["id"].as_str().unwrap());
    assert_eq!(
        f.call("GET", "/api/feed-values?after=bad", json!({}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.call("GET", &format!("{path}?user_id=other"), json!({}))
            .await
            .0,
        StatusCode::BAD_REQUEST
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
    let (_, cancelled) = f.call("POST", &format!("{path}/cancel"), json!({})).await;
    assert_eq!(cancelled["status"], "cancelled");
    assert!(cancelled["shared_content"].is_null());
    assert_eq!(
        f.call("POST", &format!("{path}/approve"), approve(&saved))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.call("POST", &format!("{path}/cancel"), json!({})).await.1,
        cancelled
    );
    f.cleanup().await;
}
#[tokio::test]
async fn value_http_checks_session_and_csrf_before_optional_runtime() {
    let (state, _, _, _, cookie, _) = retrieval_fixture().await;
    let id = Uuid::new_v4();
    let create = json!({"id":id,"connection_id":id,"connection_revision":"1","model":"fixture"});
    let consent = json!({"digest":"a".repeat(64),"acknowledge_sharing":true,"acknowledge_subscription_usage":true});
    for (method, path, body) in [
        ("GET", "/api/feed-values".into(), json!({})),
        ("GET", format!("/api/feed-values/{id}"), json!({})),
        ("GET", format!("/api/feed-values/{id}/audit"), json!({})),
        ("POST", "/api/feed-values".into(), create),
        ("POST", format!("/api/feed-values/{id}/approve"), consent),
        ("POST", format!("/api/feed-values/{id}/cancel"), json!({})),
    ] {
        for (session, csrf, expected) in [
            (None, true, 401),
            (
                Some(cookie.as_str()),
                false,
                if method == "POST" { 403 } else { 503 },
            ),
            (Some(cookie.as_str()), true, 503),
        ] {
            let response = auth_request(
                router(state.clone()),
                method,
                &path,
                session,
                &body.to_string(),
                csrf,
            )
            .await;
            assert_eq!(response.status().as_u16(), expected);
            assert_eq!(response.headers()["cache-control"], "no-store");
        }
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn value_http_pages_metadata_bounds_bodies_and_clears_expired_content() {
    let (f, connection) = setup().await;
    for _ in 0..20 {
        let (_, saved) = f
            .call("POST", "/api/feed-values", preview(&connection))
            .await;
        f.call(
            "POST",
            &format!("/api/feed-values/{}/cancel", saved["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    }
    sqlx::query("UPDATE feed_value_reviews SET created_ms=created_ms-86400000,expires_ms=expires_ms-86400000 WHERE user_id=$1").bind(Uuid::parse_str(f.owner.as_str()).unwrap()).execute(&f.pool).await.unwrap();
    let (_, saved) = f
        .call("POST", "/api/feed-values", preview(&connection))
        .await;
    let (_, page) = f.call("GET", "/api/feed-values", json!({})).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 20);
    let (_, last) = f
        .call(
            "GET",
            &format!(
                "/api/feed-values?after={}",
                page["next_cursor"].as_str().unwrap()
            ),
            json!({}),
        )
        .await;
    assert_eq!(last["items"].as_array().unwrap().len(), 1);
    assert!(last["next_cursor"].is_null());
    let other = decode(
        f.send(
            "GET",
            "/api/feed-values",
            json!({}),
            Some(&f.other_cookie),
            true,
        )
        .await,
    )
    .await
    .1;
    assert_eq!(other["items"].as_array().unwrap().len(), 0);
    let mut huge = preview(&connection);
    huge["model"] = json!("x".repeat(20000));
    assert_eq!(
        f.call("POST", "/api/feed-values", huge).await.0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    sqlx::query("UPDATE feed_value_reviews SET expires_ms=created_ms+1 WHERE user_id=$1 AND id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .bind(Uuid::parse_str(saved["id"].as_str().unwrap()).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    let path = format!("/api/feed-values/{}", saved["id"].as_str().unwrap());
    let (_, expired) = f.call("GET", &path, json!({})).await;
    assert_eq!(expired["status"], "expired");
    assert!(expired["shared_content"].is_null());
    f.call("POST", "/api/auth/logout", json!({})).await;
    assert_eq!(
        f.call("GET", &path, json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    f.cleanup().await;
}
