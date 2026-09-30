use super::*;
use personal_ai_storage::{
    agent_plans::AgentPlanStore, conversations::ConversationStore, messages::MessageStore,
    tool_calls::ToolCallStore,
};
use personal_ai_storage_postgres::PostgresStore;
use std::sync::atomic::Ordering;

async fn value(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 256 * 1024).await.unwrap()).unwrap()
}
fn input() -> serde_json::Value {
    json!({"request_id":Uuid::new_v4(),"expected_revision":1,"searches":[{"query":"evidence","limit":5},{"query":"another question","limit":3},{"query":"last question","limit":1}]})
}
fn consent(plan: &serde_json::Value) -> serde_json::Value {
    json!({"plan_digest":plan["digest"],"accepted_call_limit":plan["tool_call_limit"],"acknowledge_embedding_cost":true})
}

#[tokio::test]
async fn agent_http_rejects_untrusted_plans_and_requires_session_csrf_and_storage() {
    let (state, _, dependencies, _, cookie, _) = retrieval_fixture().await;
    let app = router(state);
    let path = format!("/api/conversations/{}/agent-plans", Uuid::new_v4());
    let body = input().to_string();
    assert_eq!(
        auth_request(app.clone(), "POST", &path, None, &body, true)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        auth_request(app.clone(), "POST", &path, Some(&cookie), &body, false)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for field in ["user_id", "tool", "conversation_id", "output"] {
        let mut forged = input();
        forged[field] = json!("forged");
        assert_eq!(
            auth_request(
                app.clone(),
                "POST",
                &path,
                Some(&cookie),
                &forged.to_string(),
                true
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let mut invalid = input();
    invalid["searches"] = json!([{"query":"question","tool":"shell"}]);
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            &path,
            Some(&cookie),
            &invalid.to_string(),
            true
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    invalid["searches"] = json!([{"query":"1"},{"query":"2"},{"query":"3"},{"query":"4"}]);
    assert_eq!(
        auth_request(
            app.clone(),
            "POST",
            &path,
            Some(&cookie),
            &invalid.to_string(),
            true
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let response = auth_request(app.clone(), "POST", &path, Some(&cookie), &body, true).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        value(response).await,
        json!({"error":{"code":"agent_plans_unavailable"}})
    );
    for route in [path.clone(), format!("{path}/{}", Uuid::new_v4())] {
        assert_eq!(
            auth_request(app.clone(), "GET", &route, None, "", false)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(dependencies.calls.load(Ordering::SeqCst), 1);
}

struct Fixture {
    app: Router,
    store: Arc<PostgresStore>,
    owner: UserId,
    foreign: UserId,
    cookie: String,
    foreign_cookie: String,
    path: String,
    dependencies: Arc<IndexDependencies>,
}
impl Fixture {
    async fn new() -> Self {
        let (mut state, sessions, dependencies, owner, cookie, _) = retrieval_fixture().await;
        let store = Arc::new(
            PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
                .await
                .unwrap(),
        );
        let mut user = sessions.get_user(&owner).await.unwrap();
        user.email = format!("{}@agent-api.example", Uuid::new_v4());
        store.save_user(&user).await.unwrap();
        let foreign = User {
            id: UserId::new(Uuid::new_v4().to_string()),
            email: format!("{}@foreign-agent.example", Uuid::new_v4()),
            display_name: "Other".into(),
        };
        sessions.save_user(&foreign).await.unwrap();
        store.save_user(&foreign).await.unwrap();
        let token = "e".repeat(64);
        sessions.sessions.lock().unwrap().insert(
            format!("{:x}", Sha256::digest(token.as_bytes())),
            foreign.id.to_string(),
        );
        let conversation = store
            .create_conversation(&owner, &Uuid::new_v4().to_string(), "API 计划")
            .await
            .unwrap();
        store
            .append_message(
                &owner,
                &conversation.id,
                &Uuid::new_v4().to_string(),
                "explicit research",
            )
            .await
            .unwrap();
        state.agent_plans = Some(store.clone());
        state.tool_calls = Some(store.clone());
        state.conversations = store.clone();
        state.messages = store.clone();
        Self {
            app: router(state),
            store,
            owner,
            foreign: foreign.id,
            cookie,
            foreign_cookie: format!("personal_ai_session_v2={token}"),
            path: format!("/api/conversations/{}/agent-plans", conversation.id),
            dependencies,
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: &serde_json::Value,
        status: StatusCode,
    ) -> serde_json::Value {
        let response = auth_request(
            self.app.clone(),
            method,
            path,
            Some(&self.cookie),
            &body.to_string(),
            true,
        )
        .await;
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        value(response).await
    }
    async fn wait(&self, path: &str, status: &str) -> serde_json::Value {
        for _ in 0..200 {
            let plan = self
                .request("GET", path, &json!(null), StatusCode::OK)
                .await;
            if plan["status"] == status {
                return plan;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("plan never reached {status}");
    }
    async fn assert_approval_boundaries(&self, path: &str, preview: &serde_json::Value) {
        let approve_path = format!("{path}/approve");
        for (body, expected) in [
            (
                json!({"plan_digest":preview["digest"],"accepted_call_limit":3,"acknowledge_embedding_cost":false}),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({"plan_digest":"f".repeat(64),"accepted_call_limit":3,"acknowledge_embedding_cost":true}),
                StatusCode::CONFLICT,
            ),
            (
                json!({"plan_digest":preview["digest"],"accepted_call_limit":2,"acknowledge_embedding_cost":true}),
                StatusCode::CONFLICT,
            ),
        ] {
            assert_eq!(
                auth_request(
                    self.app.clone(),
                    "POST",
                    &approve_path,
                    Some(&self.cookie),
                    &body.to_string(),
                    true
                )
                .await
                .status(),
                expected
            );
        }
        for (method, route) in [
            ("GET", path.to_owned()),
            ("GET", self.path.clone()),
            ("POST", approve_path.clone()),
            ("POST", format!("{path}/cancel")),
        ] {
            assert_eq!(
                auth_request(
                    self.app.clone(),
                    method,
                    &route,
                    Some(&self.foreign_cookie),
                    &consent(preview).to_string(),
                    true
                )
                .await
                .status(),
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            auth_request(
                self.app.clone(),
                "POST",
                &approve_path,
                Some(&self.cookie),
                &consent(preview).to_string(),
                false
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    async fn cleanup(self) {
        let pool = sqlx::PgPool::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        for owner in [self.owner, self.foreign] {
            sqlx::query("DELETE FROM users WHERE id=$1")
                .bind(Uuid::parse_str(owner.as_str()).unwrap())
                .execute(&pool)
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn agent_http_preview_exact_approval_execution_and_replay_are_durable_and_owner_scoped() {
    let f = Fixture::new().await;
    let body = input();
    let preview = f.request("POST", &f.path, &body, StatusCode::OK).await;
    assert_eq!(preview["status"], "draft");
    assert_eq!(preview["attempted"], 0);
    assert_eq!(preview["steps"].as_array().unwrap().len(), 3);
    assert_eq!(f.dependencies.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.request("POST", &f.path, &body, StatusCode::OK).await,
        preview
    );
    let path = format!("{}/{}", f.path, preview["request_id"].as_str().unwrap());
    let approve_path = format!("{path}/approve");
    f.assert_approval_boundaries(&path, &preview).await;
    f.request(
        "POST",
        &approve_path,
        &consent(&preview),
        StatusCode::ACCEPTED,
    )
    .await;
    let finished = f.wait(&path, "succeeded").await;
    assert_eq!(finished["attempted"], 3);
    assert_eq!(
        finished["steps"][0]["output"]["hits"][0]["text"],
        "verified evidence"
    );
    assert_eq!(f.dependencies.calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        f.store.audit_tool_calls(&f.owner, None).await.unwrap().used,
        3
    );
    f.request(
        "POST",
        &approve_path,
        &consent(&preview),
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(f.dependencies.calls.load(Ordering::SeqCst), 4);
    let reopened = PostgresStore::connect_existing(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let conversation = finished["conversation_id"].as_str().unwrap();
    let restored = reopened
        .get_agent_plan(
            &f.owner,
            conversation,
            finished["request_id"].as_str().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), finished);
    let call_id = finished["steps"][0]["call_id"].as_str().unwrap();
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tools/knowledge_search")
                .header("cookie", &f.cookie)
                .header("content-type", "application/json")
                .header("x-requested-with", "personal-ai")
                .header("idempotency-key", call_id)
                .body(Body::from(
                    json!({"query":"evidence","limit":5}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(f.dependencies.calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        reopened
            .get_tool_call(&f.owner, call_id)
            .await
            .unwrap()
            .status,
        "succeeded"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn agent_http_failure_stops_remaining_steps_and_cancelled_drafts_never_execute() {
    let f = Fixture::new().await;
    let preview = f.request("POST", &f.path, &input(), StatusCode::OK).await;
    let path = format!("{}/{}", f.path, preview["request_id"].as_str().unwrap());
    let cancelled = f
        .request(
            "POST",
            &format!("{path}/cancel"),
            &json!({}),
            StatusCode::OK,
        )
        .await;
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(
        f.request(
            "POST",
            &format!("{path}/cancel"),
            &json!({}),
            StatusCode::OK
        )
        .await,
        cancelled
    );
    assert_eq!(
        auth_request(
            f.app.clone(),
            "POST",
            &format!("{path}/approve"),
            Some(&f.cookie),
            &consent(&preview).to_string(),
            true
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(f.dependencies.calls.load(Ordering::SeqCst), 1);
    let preview = f.request("POST", &f.path, &input(), StatusCode::OK).await;
    let path = format!("{}/{}", f.path, preview["request_id"].as_str().unwrap());
    f.dependencies.mode.store(1, Ordering::SeqCst);
    f.request(
        "POST",
        &format!("{path}/approve"),
        &consent(&preview),
        StatusCode::ACCEPTED,
    )
    .await;
    let failed = f.wait(&path, "failed").await;
    assert_eq!(failed["attempted"], 1);
    assert_eq!(failed["steps"][0]["status"], "failed");
    assert!(
        failed["steps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|step| step["output"].is_null())
    );
    assert_eq!(
        f.store.audit_tool_calls(&f.owner, None).await.unwrap().used,
        1
    );
    assert_eq!(f.dependencies.calls.load(Ordering::SeqCst), 2);
    f.dependencies.mode.store(0, Ordering::SeqCst);
    f.request(
        "POST",
        &format!("{path}/approve"),
        &consent(&preview),
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(f.dependencies.calls.load(Ordering::SeqCst), 2);
    f.cleanup().await;
}
