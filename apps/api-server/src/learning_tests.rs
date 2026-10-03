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
    assert_eq!(plan["evidence_reviews"][0]["state"], "not_recorded");
    assert_eq!(plan["evidence_reviews"][0]["skill_revision"], "1");
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
    assert_eq!(saved["evidence_reviews"][0]["state"], "unverified");
    assert_eq!(
        saved["evidence_reviews"][0]["result_request_id"],
        result["request_id"]
    );
    assert_eq!(saved["evidence_reviews"][0]["task_id"], task);
    assert_eq!(f.call("GET", &plan_path, json!({})).await.1, saved);
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
    assert_eq!(erased["evidence_reviews"], json!([]));
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
        (
            "POST",
            format!("/api/learning/plans/{id}/tasks/{id}/evidence"),
            json!({"request_id":id,"body":{"explanation":"概念","work":"产物","verification":"验证","limitations":"局限"}}),
        ),
        (
            "DELETE",
            format!("/api/learning/plans/{id}/tasks/{id}/evidence"),
            json!({"request_id":id}),
        ),
        ("GET", "/api/learning/snapshot".to_owned(), json!({})),
        ("GET", "/api/learning/progress".to_owned(), json!({})),
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

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_progress_is_session_scoped_read_only_and_exact() {
    let f = Fixture::new().await;
    let path = "/api/learning/progress";
    assert_eq!(
        f.send("GET", path, json!({}), None, false).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let id = Uuid::new_v4();
    f.call(
        "PUT",
        &format!("/api/learning/skills/{id}"),
        json!({"revision":"0","name":"私有进度技能","enabled":true,"prerequisite_ids":[]}),
    )
    .await;
    sqlx::query("UPDATE learning_state SET revision=9007199254740993 WHERE user_id=$1")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    let response = f.send("GET", path, json!({}), Some(&f.cookie), false).await;
    assert_eq!(response.headers()["cache-control"], "no-store");
    let (status, p) = decode(response).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["enabled_skills"], 1);
    assert_eq!(p["revision"], "9007199254740993");
    assert_eq!(p["timezone"], "UTC");
    assert!(p["as_of_unix_ms"].is_string());
    assert!(p["day_start_unix_ms"].is_string());
    assert!(p["day_end_unix_ms"].is_string());
    assert!(!p.to_string().contains("私有进度技能"));
    let (_, other) = decode(
        f.send("GET", path, json!({}), Some(&f.other_cookie), false)
            .await,
    )
    .await;
    assert_eq!(other["enabled_skills"], 0);
    assert_eq!(
        f.call("GET", &format!("{path}?user={}", f.other), json!({}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.call("POST", path, json!({})).await.0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        f.store.learning_snapshot(&f.owner).await.unwrap().revision,
        9_007_199_254_740_993
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn learning_evidence_is_private_immutable_version_bound_and_erased_without_revival() {
    let f = Fixture::new().await;
    let skill = Uuid::new_v4();
    let skill_path = format!("/api/learning/skills/{skill}");
    f.call(
        "PUT",
        &skill_path,
        json!({"revision":"0","name":"证据技能","enabled":true,"prerequisite_ids":[]}),
    )
    .await;
    let plan_id = Uuid::new_v4();
    let plan_path = format!("/api/learning/plans/{plan_id}");
    let (_, plan) = f.call("POST", "/api/learning/plans", json!({"request_id":plan_id,"expected_revision":"1","budget_minutes":30,"goal_skill_ids":[skill]})).await;
    let task = plan["plan"]["tasks"][0]["task_id"].as_str().unwrap();
    let path = format!("{plan_path}/tasks/{task}/evidence");
    let input = json!({"request_id":Uuid::new_v4(),"body":{"explanation":"解释 <script>","work":"自己的解答","verification":"执行检查，结果通过","limitations":"尚未验证边界情况"}});
    for method in ["POST", "DELETE"] {
        assert_eq!(
            f.send(method, &path, input.clone(), None, true)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            f.send(method, &path, input.clone(), Some(&f.cookie), false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        f.call("POST", &path, input.clone()).await.0,
        StatusCode::CONFLICT
    );
    f.call(
        "POST",
        &format!("{plan_path}/tasks/{task}/result"),
        json!({"request_id":Uuid::new_v4(),"outcome":"completed","note":"","actual_minutes":5}),
    )
    .await;
    for text in ["", "   ", "bad\u{0000}"] {
        let mut bad = input.clone();
        bad["body"]["verification"] = json!(text);
        assert_eq!(f.call("POST", &path, bad).await.0, StatusCode::BAD_REQUEST);
    }
    let mut bad = input.clone();
    bad["body"]["work"] = json!("字".repeat(2001));
    assert_eq!(f.call("POST", &path, bad).await.0, StatusCode::BAD_REQUEST);
    let mut bad = input.clone();
    bad["body"]["score"] = json!(100);
    assert_eq!(
        f.call("POST", &path, bad).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        f.send("POST", &path, input.clone(), Some(&f.other_cookie), true)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let (a, b) = tokio::join!(
        f.call("POST", &path, input.clone()),
        f.call("POST", &path, input.clone())
    );
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(a, b);
    let saved = a.1;
    assert_eq!(saved["results"][0]["evidence"]["body"], input["body"]);
    assert!(saved["results"][0]["evidence"]["created_at_unix_ms"].is_string());
    assert_eq!(saved["evidence_reviews"][0]["state"], "unverified");
    assert_eq!(saved["plan"], plan["plan"]);
    assert_eq!(f.call("GET", &plan_path, json!({})).await.1, saved);
    let (_, snapshot) = f.call("GET", "/api/learning/snapshot", json!({})).await;
    assert_eq!(snapshot["revision"], "1");
    assert_eq!(snapshot["assessments"], json!([]));
    let mut changed = input.clone();
    changed["body"]["work"] = json!("不同材料");
    assert_eq!(f.call("POST", &path, changed).await.0, StatusCode::CONFLICT);
    let delete = json!({"request_id":input["request_id"]});
    assert_eq!(
        f.send("DELETE", &path, delete.clone(), Some(&f.other_cookie), true)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.call("DELETE", &path, json!({"request_id":Uuid::new_v4()}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (removed, replay) = tokio::join!(
        f.call("DELETE", &path, delete.clone()),
        f.call("POST", &path, input.clone())
    );
    assert_eq!(removed.0, StatusCode::OK);
    assert!(matches!(replay.0, StatusCode::OK | StatusCode::CONFLICT));
    let erased = removed.1;
    assert_eq!(erased["results"][0]["evidence"]["deleted"], true);
    assert!(erased["results"][0]["evidence"]["body"].is_null());
    assert_eq!(erased["evidence_reviews"][0]["state"], "missing_note");
    assert_eq!(f.call("DELETE", &path, delete).await.1, erased);
    assert_eq!(
        f.call("POST", &path, input.clone()).await.0,
        StatusCode::CONFLICT
    );
    let mut fresh = input.clone();
    fresh["request_id"] = json!(Uuid::new_v4());
    assert_eq!(f.call("POST", &path, fresh).await.0, StatusCode::CONFLICT);
    let body: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT body FROM learning_evidence WHERE user_id=$1 AND task_id=$2")
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
            .bind(Uuid::parse_str(task).unwrap())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(body.is_none());
    f.call("DELETE", &skill_path, json!({"revision":"1"})).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM learning_evidence WHERE user_id=$1")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(f.call("POST", &path, input).await.0, StatusCode::CONFLICT);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_evidence_rejects_cancelled_results_and_changed_skill_versions() {
    let f = Fixture::new().await;
    for outcome in ["cancelled", "completed"] {
        let skill = Uuid::new_v4();
        let skill_path = format!("/api/learning/skills/{skill}");
        f.call(
            "PUT",
            &skill_path,
            json!({"revision":"0","name":"版本技能","enabled":true,"prerequisite_ids":[]}),
        )
        .await;
        let (_, snapshot) = f.call("GET", "/api/learning/snapshot", json!({})).await;
        let plan = Uuid::new_v4();
        let (_, saved) = f.call("POST", "/api/learning/plans", json!({"request_id":plan,"expected_revision":snapshot["revision"],"budget_minutes":30,"goal_skill_ids":[skill]})).await;
        let task = saved["plan"]["tasks"][0]["task_id"].as_str().unwrap();
        let path = format!("/api/learning/plans/{plan}/tasks/{task}");
        f.call("POST", &format!("{path}/result"), json!({"request_id":Uuid::new_v4(),"outcome":outcome,"note":"记录","actual_minutes":if outcome=="completed" {5} else {0}})).await;
        if outcome == "completed" {
            f.call(
                "PUT",
                &skill_path,
                json!({"revision":"1","name":"修改后技能","enabled":true,"prerequisite_ids":[]}),
            )
            .await;
        }
        assert_eq!(f.call("POST", &format!("{path}/evidence"), json!({"request_id":Uuid::new_v4(),"body":{"explanation":"概念","work":"产物","verification":"验证","limitations":"局限"}})).await.0, StatusCode::CONFLICT);
    }
    f.cleanup().await;
}
