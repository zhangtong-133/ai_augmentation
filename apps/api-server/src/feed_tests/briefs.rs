use super::*;

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn briefs_require_sessions_csrf_strict_input_and_private_no_store_responses() {
    let f = Fixture::new(false).await;
    let key = Uuid::new_v4();
    let paths = [
        "/api/feed-brief-preferences".to_owned(),
        "/api/feed-briefs".to_owned(),
        format!("/api/feed-briefs/{key}"),
    ];
    for path in &paths {
        let r = f.send("GET", path, json!({}), None, false).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(r.headers()["cache-control"], "no-store");
    }
    for (method, path, body) in [
        (
            "PUT",
            paths[0].as_str(),
            json!({"revision":"0","keywords":[]}),
        ),
        (
            "POST",
            paths[1].as_str(),
            json!({"request_id":key,"preference_revision":"0"}),
        ),
        ("DELETE", paths[2].as_str(), json!({})),
    ] {
        assert_eq!(
            f.send(method, path, body, Some(&f.cookie), false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    for body in [
        json!({"revision":0,"keywords":[]}),
        json!({"revision":"00","keywords":[]}),
        json!({"revision":"-1","keywords":[]}),
        json!({"revision":"0","keywords":["Rust","rust"]}),
        json!({"revision":"0","keywords":[],"owner":"forged"}),
    ] {
        assert!(f.call("PUT", &paths[0], body).await.0.is_client_error());
    }
    for body in [
        json!({"request_id":key,"preference_revision":"0","candidates":[]}),
        json!({"request_id":Uuid::nil(),"preference_revision":"0"}),
        json!({"request_id":key,"preference_revision":"9223372036854775808"}),
    ] {
        assert!(f.call("POST", &paths[1], body).await.0.is_client_error());
    }
    for path in [
        "/api/feed-briefs?after=bad",
        "/api/feed-briefs?user_id=forged",
        "/api/feed-briefs/not-a-uuid",
    ] {
        assert_eq!(
            f.call("GET", path, json!({})).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let huge = json!({"revision":"0","keywords":["x".repeat(17000)]});
    assert_eq!(
        f.call("PUT", &paths[0], huge).await.0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(f.transport.calls.load(Ordering::SeqCst), 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn briefs_freeze_saved_entries_use_exact_versions_and_clear_deleted_sources() {
    let f = Fixture::new(true).await;
    let sub = f.sub().await;
    let draft = f.preview(&sub).await;
    let confirm = format!(
        "/api/feed-collections/{}/confirm",
        draft["plan"]["request_id"].as_str().unwrap()
    );
    assert_eq!(
        f.call(
            "POST",
            &confirm,
            json!({"accepted_digest":draft["digest"],"acknowledge_source_request":true})
        )
        .await
        .0,
        StatusCode::OK
    );
    let prefpath = "/api/feed-brief-preferences";
    assert_eq!(
        f.call("GET", prefpath, json!({})).await.1,
        json!({"revision":"0","keywords":[]})
    );
    let (code, pref) = f
        .call(
            "PUT",
            prefpath,
            json!({"revision":"0","keywords":[" Title "]}),
        )
        .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(pref, json!({"revision":"1","keywords":["title"]}));
    assert_eq!(
        f.call("PUT", prefpath, json!({"revision":"0","keywords":[]}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let key = Uuid::new_v4();
    let body = json!({"request_id":key,"preference_revision":"1"});
    let (code, saved) = f.call("POST", "/api/feed-briefs", body.clone()).await;
    assert_eq!(code, StatusCode::CREATED);
    assert_eq!(saved["preference_revision"], "1");
    assert!(saved["day_start_unix_ms"].is_string());
    assert!(saved["plan"]["items"][0]["entry"]["first_seen_unix_ms"].is_string());
    assert_eq!(saved["plan"]["items"][0]["entry"]["title"], "Title");
    assert_eq!(saved["plan"]["items"][0]["matches"][0]["points"], 12);
    assert_eq!(
        f.call("PUT", prefpath, json!({"revision":"1","keywords":["new"]}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        f.call("POST", "/api/feed-briefs", body.clone()).await.1,
        saved
    );
    let path = format!("/api/feed-briefs/{key}");
    let detail = f
        .send("GET", &path, json!({}), Some(&f.cookie), false)
        .await;
    assert_eq!(detail.headers()["cache-control"], "no-store");
    assert_eq!(decode(detail).await.1, saved);
    for method in ["GET", "DELETE"] {
        assert_eq!(
            f.send(method, &path, json!({}), Some(&f.other_cookie), true)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    let (_, other) = decode(
        f.send(
            "GET",
            "/api/feed-briefs",
            json!({}),
            Some(&f.other_cookie),
            false,
        )
        .await,
    )
    .await;
    assert_eq!(other["items"], json!([]));
    let (_, list) = f.call("GET", "/api/feed-briefs", json!({})).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert!(list["items"][0].get("plan").is_none());
    let subpath = format!(
        "/api/feed-subscriptions/{}",
        sub["snapshot"]["subscription_id"].as_str().unwrap()
    );
    assert_eq!(
        f.call("DELETE", &subpath, json!({"revision":"1"})).await.0,
        StatusCode::OK
    );
    let (_, invalidated) = f.call("POST", "/api/feed-briefs", body).await;
    assert_eq!(invalidated["status"], "invalidated");
    assert!(invalidated["plan"].is_null());
    assert_eq!(f.call("DELETE", &path, json!({})).await.0, StatusCode::OK);
    assert_eq!(f.call("DELETE", &path, json!({})).await.0, StatusCode::OK);
    assert_eq!(f.call("GET", &path, json!({})).await.1["status"], "deleted");
    // 只有显式采集访问了传输，日报读写没有新增调用。
    assert_eq!(f.transport.calls.load(Ordering::SeqCst), 1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn briefs_work_with_collection_disabled_and_preserve_large_revisions_exactly() {
    let f = Fixture::new(false).await;
    f.call(
        "PUT",
        "/api/feed-brief-preferences",
        json!({"revision":"0","keywords":[]}),
    )
    .await;
    sqlx::query("UPDATE feed_brief_preferences SET revision=9007199254740993 WHERE user_id=$1")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.call("GET", "/api/feed-brief-preferences", json!({}))
            .await
            .1["revision"],
        "9007199254740993"
    );
    let body = json!({"request_id":Uuid::new_v4(),"preference_revision":"9007199254740993"});
    let (code, saved) = f.call("POST", "/api/feed-briefs", body).await;
    assert_eq!(code, StatusCode::CREATED);
    assert_eq!(saved["preference_revision"], "9007199254740993");
    assert_eq!(saved["plan"]["items"], json!([]));
    assert_eq!(f.transport.calls.load(Ordering::SeqCst), 0);
    f.cleanup().await;
}
