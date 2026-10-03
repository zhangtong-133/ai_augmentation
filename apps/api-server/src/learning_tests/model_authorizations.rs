use super::reviews::{evidence, setup};
use super::*;
use personal_ai_storage::subscription_connections::{
    SubscriptionConnectionStore, VerifiedSubscriptionConnection,
};
async fn connection(f: &Fixture) -> Uuid {
    let key = Uuid::new_v4();
    let now: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    f.store
        .save_subscription_connection(
            &f.owner,
            &key.to_string(),
            0,
            &VerifiedSubscriptionConnection {
                host_id: Uuid::new_v4().urn().to_string(),
                client_id: format!("oaiapp_{}", Uuid::new_v4().simple()),
                subject: "fixture".into(),
                label: "local-test".into(),
                models: vec!["fixture-model".into()],
                valid_until_unix_ms: now + 3_600_000,
            },
        )
        .await
        .unwrap();
    key
}
async fn draft(f: &Fixture, path: &str, key: Uuid) -> serde_json::Value {
    let (status,item)=f.call("POST",&format!("{path}/model-authorizations"),json!({"request_id":Uuid::new_v4(),"connection_id":key,"connection_revision":"1","model":"fixture-model"})).await;
    assert_eq!(status, StatusCode::OK, "{item}");
    item
}
fn consent(item: &serde_json::Value) -> serde_json::Value {
    json!({"digest":item["digest"],"acknowledge_sharing":true,"acknowledge_subscription_usage":true})
}
fn endpoint(item: &serde_json::Value) -> String {
    format!(
        "/api/learning/model-authorizations/{}",
        item["request_id"].as_str().unwrap()
    )
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_authorizations_bind_exact_consent_and_never_dispatch_or_change_scores() {
    let (f, plan, path, _) = setup().await;
    evidence(&f, &path).await;
    let key = connection(&f).await;
    let before = f.call("GET", &plan, json!({})).await.1;
    let snapshot = f.call("GET", "/api/learning/snapshot", json!({})).await.1;
    let item = draft(&f, &path, key).await;
    let url = endpoint(&item);
    assert_eq!(item["status"], "draft");
    assert!(item["preview"].is_object());
    assert!(item["approved_at_unix_ms"].is_null());
    let input = json!({"request_id":item["request_id"],"connection_id":key,"connection_revision":"1","model":"fixture-model"});
    assert_eq!(
        f.call(
            "POST",
            &format!("{path}/model-authorizations"),
            input.clone()
        )
        .await
        .1,
        item
    );
    let mut changed = input.clone();
    changed["model"] = json!("other");
    assert_eq!(
        f.call("POST", &format!("{path}/model-authorizations"), changed)
            .await
            .0,
        StatusCode::CONFLICT
    );
    for field in ["acknowledge_sharing", "acknowledge_subscription_usage"] {
        let mut c = consent(&item);
        c[field] = json!(false);
        assert_eq!(
            f.call("POST", &format!("{url}/approve"), c).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut c = consent(&item);
    c["digest"] = json!("a".repeat(64));
    assert_eq!(
        f.call("POST", &format!("{url}/approve"), c).await.0,
        StatusCode::CONFLICT
    );
    let approve_path = format!("{url}/approve");
    let (a, b) = tokio::join!(
        f.call("POST", &approve_path, consent(&item)),
        f.call("POST", &approve_path, consent(&item))
    );
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(a.1, b.1);
    assert_eq!(a.1["status"], "authorized");
    assert_eq!(f.call("GET", &plan, json!({})).await.1, before);
    assert_eq!(
        f.call("GET", "/api/learning/snapshot", json!({})).await.1,
        snapshot
    );
    let cancelled = f.call("POST", &format!("{url}/cancel"), json!({})).await.1;
    assert_eq!(cancelled["status"], "cancelled");
    assert!(cancelled["preview"].is_null());
    assert_eq!(
        f.call("POST", &format!("{url}/cancel"), json!({})).await.1,
        cancelled
    );
    assert_eq!(
        f.call("POST", &format!("{url}/approve"), consent(&item))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.call("POST", &format!("{path}/model-authorizations"), input)
            .await
            .1,
        cancelled
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM learning_model_authorization_audit WHERE user_id=$1",
    )
    .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(count, 3);
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_authorizations_require_session_csrf_strict_input_and_owner_bound_connection() {
    let (f, _, path, _) = setup().await;
    evidence(&f, &path).await;
    let key = connection(&f).await;
    let item = draft(&f, &path, key).await;
    let url = endpoint(&item);
    for (method, route, body) in [
        ("GET", url.clone(), json!({})),
        ("POST", format!("{url}/approve"), consent(&item)),
        ("POST", format!("{url}/cancel"), json!({})),
    ] {
        for (cookie, status) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some(f.other_cookie.as_str()), StatusCode::NOT_FOUND),
        ] {
            let response = f.send(method, &route, body.clone(), cookie, true).await;
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()["cache-control"], "no-store");
        }
        if method == "POST" {
            assert_eq!(
                f.send(method, &route, body, Some(&f.cookie), false)
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
    }
    let mut c = consent(&item);
    c["amount"] = json!(0);
    assert_eq!(
        f.call("POST", &format!("{url}/approve"), c).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        f.call("GET", &format!("{url}?include_credentials=true"), json!({}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    for (connection, revision, model) in [
        (Uuid::new_v4(), "1", "fixture-model"),
        (key, "2", "fixture-model"),
        (key, "1", "not-allowed"),
    ] {
        assert_eq!(f.call("POST",&format!("{path}/model-authorizations"),json!({"request_id":Uuid::new_v4(),"connection_id":connection,"connection_revision":revision,"model":model})).await.0,StatusCode::CONFLICT);
    }
    let other = f
        .send(
            "GET",
            "/api/learning/model-authorizations",
            json!({}),
            Some(&f.other_cookie),
            false,
        )
        .await;
    assert_eq!(decode(other).await.1["items"], json!([]));
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_authorizations_invalidate_on_sources_connections_and_expiry_without_revival() {
    for mode in ["evidence", "skill", "plan", "connection", "expiry"] {
        let (f, plan, path, skill) = setup().await;
        let evidence = evidence(&f, &path).await;
        let key = connection(&f).await;
        let item = draft(&f, &path, key).await;
        let url = endpoint(&item);
        f.call("POST", &format!("{url}/approve"), consent(&item))
            .await;
        match mode {
            "evidence" => {
                f.call("DELETE", &path, json!({"request_id":evidence}))
                    .await;
            }
            "skill" => {
                f.call(
                    "PUT",
                    &format!("/api/learning/skills/{skill}"),
                    json!({"revision":"1","name":"新版本","enabled":true,"prerequisite_ids":[]}),
                )
                .await;
            }
            "plan" => {
                f.call("DELETE", &plan, json!({})).await;
            }
            "connection" => {
                f.store
                    .revoke_subscription_connection(&f.owner, &key.to_string(), 1)
                    .await
                    .unwrap();
            }
            _ => {
                sqlx::query("UPDATE learning_model_authorizations SET approved_ms=created_ms-300001,created_ms=created_ms-300001,expires_ms=created_ms-1 WHERE user_id=$1").bind(Uuid::parse_str(f.owner.as_str()).unwrap()).execute(&f.pool).await.unwrap();
            }
        }
        let stored = f.call("GET", &url, json!({})).await.1;
        assert_eq!(
            stored["status"],
            if mode == "expiry" {
                "expired"
            } else {
                "invalidated"
            },
            "{mode}"
        );
        assert!(stored["preview"].is_null());
        assert_eq!(
            f.call("POST", &format!("{url}/approve"), consent(&item))
                .await
                .0,
            StatusCode::CONFLICT
        );
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_authorization_quota_paging_and_cancel_race_preserve_request_identity() {
    let (f, _, path, _) = setup().await;
    evidence(&f, &path).await;
    let key = connection(&f).await;
    let item = draft(&f, &path, key).await;
    let url = endpoint(&item);
    let approve = format!("{url}/approve");
    let cancel = format!("{url}/cancel");
    let (a, c) = tokio::join!(
        f.call("POST", &approve, consent(&item)),
        f.call("POST", &cancel, json!({}))
    );
    assert!(matches!(a.0, StatusCode::OK | StatusCode::CONFLICT));
    assert_eq!(c.0, StatusCode::OK);
    assert_eq!(
        f.call("GET", &url, json!({})).await.1["status"],
        "cancelled"
    );
    for _ in 1..20 {
        let item = draft(&f, &path, key).await;
        f.call("POST", &format!("{}/cancel", endpoint(&item)), json!({}))
            .await;
    }
    assert_eq!(f.call("POST",&format!("{path}/model-authorizations"),json!({"request_id":Uuid::new_v4(),"connection_id":key,"connection_revision":"1","model":"fixture-model"})).await.0,StatusCode::CONFLICT);
    // Advance only fixture metadata to the preceding UTC day; cancelled records cannot expose content.
    sqlx::query("UPDATE learning_model_authorizations SET approved_ms=NULL,created_ms=created_ms-86400000,expires_ms=expires_ms-86400000 WHERE user_id=$1").bind(Uuid::parse_str(f.owner.as_str()).unwrap()).execute(&f.pool).await.unwrap();
    draft(&f, &path, key).await;
    let page = f
        .call("GET", "/api/learning/model-authorizations", json!({}))
        .await
        .1;
    assert_eq!(page["items"].as_array().unwrap().len(), 20);
    let next = page["next_cursor"].as_str().unwrap();
    let last = f
        .call(
            "GET",
            &format!("/api/learning/model-authorizations?after={next}"),
            json!({}),
        )
        .await
        .1;
    assert_eq!(last["items"].as_array().unwrap().len(), 1);
    assert!(last["next_cursor"].is_null());
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_authorization_audit_failure_rolls_back_approval() {
    let (f, _, path, _) = setup().await;
    evidence(&f, &path).await;
    let key = connection(&f).await;
    let item = draft(&f, &path, key).await;
    let constraint = format!("fail_learning_auth_{}", Uuid::new_v4().simple());
    let owner = Uuid::parse_str(f.owner.as_str()).unwrap();
    sqlx::query(&format!("ALTER TABLE learning_model_authorization_audit ADD CONSTRAINT {constraint} CHECK(user_id <> '{owner}'::uuid OR event <> 'authorized')")).execute(&f.pool).await.unwrap();
    let status = f
        .call(
            "POST",
            &format!("{}/approve", endpoint(&item)),
            consent(&item),
        )
        .await
        .0;
    sqlx::query(&format!(
        "ALTER TABLE learning_model_authorization_audit DROP CONSTRAINT {constraint}"
    ))
    .execute(&f.pool)
    .await
    .unwrap();
    assert!(!status.is_success());
    let current = f.call("GET", &endpoint(&item), json!({})).await.1;
    assert_eq!(current["status"], "draft");
    assert!(current["approved_at_unix_ms"].is_null());
    f.cleanup().await;
}
