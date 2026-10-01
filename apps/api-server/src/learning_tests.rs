use super::*;
use personal_ai_storage::{MetadataStore, learning::LearningStore};
use personal_ai_storage_postgres::PostgresStore;
struct Fixture {
    state: AppState,
    store: Arc<PostgresStore>,
    pool: sqlx::PgPool,
    owner: UserId,
    other: UserId,
    cookie: String,
    other_cookie: String,
}
impl Fixture {
    async fn new() -> Self {
        let (mut state, sessions, _, owner, cookie, _) = retrieval_fixture().await;
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let store = Arc::new(PostgresStore::connect(&url).await.unwrap());
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let mut user = sessions.get_user(&owner).await.unwrap();
        user.email = format!("{}@learning-http.example", Uuid::new_v4());
        store.save_user(&user).await.unwrap();
        let other = User {
            id: UserId::new(Uuid::new_v4().to_string()),
            email: format!("{}@learning-http.example", Uuid::new_v4()),
            display_name: "Other".into(),
        };
        sessions.save_user(&other).await.unwrap();
        store.save_user(&other).await.unwrap();
        let token = "e".repeat(64);
        sessions.sessions.lock().unwrap().insert(
            format!("{:x}", Sha256::digest(token.as_bytes())),
            other.id.to_string(),
        );
        state.learning = Some(store.clone());
        Self {
            state,
            store,
            pool,
            owner,
            other: other.id,
            cookie,
            other_cookie: format!("personal_ai_session_v2={token}"),
        }
    }
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: serde_json::Value,
        cookie: Option<&str>,
        csrf: bool,
    ) -> axum::response::Response {
        auth_request(
            router(self.state.clone()),
            method,
            path,
            cookie,
            &body.to_string(),
            csrf,
        )
        .await
    }
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        decode(
            self.send(method, path, body, Some(&self.cookie), true)
                .await,
        )
        .await
    }
    async fn cleanup(&self) {
        sqlx::query("DELETE FROM users WHERE id=$1 OR id=$2")
            .bind(Uuid::parse_str(self.owner.as_str()).unwrap())
            .bind(Uuid::parse_str(self.other.as_str()).unwrap())
            .execute(&self.pool)
            .await
            .unwrap();
    }
}
async fn decode(response: axum::response::Response) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn learning_http_auth_validation_idempotency_and_result_cleanup() {
    let f = Fixture::new().await;
    let skill = Uuid::new_v4();
    let path = format!("/api/learning/skills/{skill}");
    let input = json!({"revision":"0","name":"Rust <script>","enabled":true,"prerequisite_ids":[]});
    for (cookie, csrf, expected) in [(None, true, 401), (Some(f.cookie.as_str()), false, 403)] {
        let response = f.send("PUT", &path, input.clone(), cookie, csrf).await;
        assert_eq!(response.status().as_u16(), expected);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let (status, node) = f.call("PUT", &path, input.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(node["revision"], "1");
    assert_eq!(f.call("PUT", &path, input.clone()).await.1, node);
    for version in [
        json!(0),
        json!("00"),
        json!("-1"),
        json!("9223372036854775808"),
    ] {
        let mut bad = input.clone();
        bad["revision"] = version;
        assert!(f.call("PUT", &path, bad).await.0.is_client_error());
    }
    let mut bad = input.clone();
    bad["authority"] = json!(true);
    assert_eq!(
        f.call("PUT", &path, bad).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        f.call("GET", "/api/learning/plans?unknown=1", json!({}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let assessment = json!({"request_id":Uuid::new_v4(),"expected_revision":"1","skill_id":skill,"skill_revision":"1","score":40});
    let (_, rating) = f
        .call("POST", "/api/learning/assessments", assessment.clone())
        .await;
    assert!(rating["assessed_at_unix_ms"].is_string());
    assert_eq!(
        f.call("POST", "/api/learning/assessments", assessment)
            .await
            .1,
        rating
    );
    let plan_id = Uuid::new_v4();
    let plan_path = format!("/api/learning/plans/{plan_id}");
    let input = json!({"request_id":plan_id,"expected_revision":"2","budget_minutes":30,"goal_skill_ids":[skill]});
    let (status, plan) = f.call("POST", "/api/learning/plans", input.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(plan["snapshot_revision"], "2");
    assert_eq!(plan["plan"]["tasks"][0]["skill_revision"], "1");
    assert_eq!(f.call("POST", "/api/learning/plans", input).await.1, plan);
    let task = plan["plan"]["tasks"][0]["task_id"].as_str().unwrap();
    let result_path = format!("{plan_path}/tasks/{task}/result");
    let result = json!({"request_id":Uuid::new_v4(),"outcome":"completed","note":"私有结果 <img>","actual_minutes":25});
    assert_eq!(
        f.send("POST", &result_path, result.clone(), Some(&f.cookie), false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.send(
            "POST",
            &result_path,
            result.clone(),
            Some(&f.other_cookie),
            true
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let (status, saved) = f.call("POST", &result_path, result.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["plan"], plan["plan"]);
    assert_eq!(saved["digest"], plan["digest"]);
    assert_eq!(saved["results"][0]["note"], "私有结果 <img>");
    assert!(saved["results"][0]["recorded_at_unix_ms"].is_string());
    assert_eq!(f.call("POST", &result_path, result.clone()).await.1, saved);
    let mut changed = result.clone();
    changed["note"] = json!("different");
    assert_eq!(
        f.call("POST", &result_path, changed).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.call("GET", "/api/learning/snapshot", json!({})).await.1["revision"],
        "2"
    );
    assert_eq!(
        f.send("GET", &plan_path, json!({}), Some(&f.other_cookie), false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let (_, other) = decode(
        f.send(
            "GET",
            "/api/learning/snapshot",
            json!({}),
            Some(&f.other_cookie),
            false,
        )
        .await,
    )
    .await;
    assert_eq!(other["skills"], json!([]));
    assert_eq!(
        f.call("DELETE", &path, json!({"revision":"1"})).await.0,
        StatusCode::OK
    );
    let (_, erased) = f.call("GET", &plan_path, json!({})).await;
    assert_eq!(erased["status"], "invalidated");
    assert!(erased["plan"].is_null());
    assert_eq!(erased["results"], json!([]));
    assert_eq!(
        f.call("POST", &result_path, result).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.call("DELETE", &plan_path, json!({})).await.0,
        StatusCode::OK
    );
    assert_eq!(
        f.call("GET", &plan_path, json!({})).await.1["status"],
        "deleted"
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_http_preserves_large_revision_strings_and_rejects_client_plans() {
    let f = Fixture::new().await;
    let skill = Uuid::new_v4();
    let path = format!("/api/learning/skills/{skill}");
    f.call(
        "PUT",
        &path,
        json!({"revision":"0","name":"精确版本","enabled":true,"prerequisite_ids":[]}),
    )
    .await;
    sqlx::query("UPDATE learning_skills SET revision=9007199254740993 WHERE user_id=$1 AND id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .bind(skill)
        .execute(&f.pool)
        .await
        .unwrap();
    let (status, saved) = f.call("PUT", &path, json!({"revision":"9007199254740993","name":"新版本","enabled":true,"prerequisite_ids":[]})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["revision"], "9007199254740994");
    let (status, _) = f.call("POST", "/api/learning/plans", json!({"request_id":Uuid::new_v4(),"expected_revision":"2","budget_minutes":30,"goal_skill_ids":[skill],"plan":{}})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        f.store.learning_snapshot(&f.owner).await.unwrap().revision,
        2
    );
    f.cleanup().await;
}

#[tokio::test]
async fn learning_http_guards_every_route_before_using_optional_store() {
    let (state, _, _, _, cookie, _) = retrieval_fixture().await;
    let id = Uuid::new_v4();
    for (method, path, body) in [
        ("GET", "/api/learning/snapshot".to_owned(), json!({})),
        ("GET", "/api/learning/plans".to_owned(), json!({})),
        ("GET", format!("/api/learning/plans/{id}"), json!({})),
        (
            "PUT",
            format!("/api/learning/skills/{id}"),
            json!({"revision":"0","name":"Rust","enabled":true,"prerequisite_ids":[]}),
        ),
        (
            "DELETE",
            format!("/api/learning/skills/{id}"),
            json!({"revision":"1"}),
        ),
        (
            "POST",
            "/api/learning/assessments".to_owned(),
            json!({"request_id":id,"expected_revision":"1","skill_id":id,"skill_revision":"1","score":50}),
        ),
        (
            "POST",
            "/api/learning/plans".to_owned(),
            json!({"request_id":id,"expected_revision":"1","budget_minutes":30,"goal_skill_ids":[id]}),
        ),
        ("DELETE", format!("/api/learning/plans/{id}"), json!({})),
        (
            "POST",
            format!("/api/learning/plans/{id}/tasks/{id}/result"),
            json!({"request_id":id,"outcome":"cancelled","note":"","actual_minutes":0}),
        ),
    ] {
        let raw = body.to_string();
        let response = auth_request(router(state.clone()), method, &path, None, &raw, true).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()["cache-control"], "no-store");
        if method != "GET" {
            assert_eq!(
                auth_request(
                    router(state.clone()),
                    method,
                    &path,
                    Some(&cookie),
                    &raw,
                    false
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
        }
        let response = auth_request(
            router(state.clone()),
            method,
            &path,
            Some(&cookie),
            &raw,
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            decode(response).await.1["error"]["code"],
            "learning_unavailable"
        );
    }
}
