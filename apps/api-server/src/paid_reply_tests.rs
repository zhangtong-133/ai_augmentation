use super::*;
use crate::{AuthConfig, router};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use personal_ai_agent_core::{
    BoxFuture,
    reply_executor::{ReplyCompletion, ReplySendError, ReplySender},
};
use personal_ai_domain::User;
use personal_ai_llm_openai::replies::{OpenAiReplyPolicy, REPLY_MODEL, ReplyPrices};
use personal_ai_storage::{
    MetadataStore,
    conversations::ConversationStore,
    messages::MessageStore,
    reply_budgets::{ReplyBudgetPlanner, ReplyUsage},
};
use personal_ai_storage_postgres::PostgresStore;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

struct Sender(AtomicUsize);
impl ReplySender for Sender {
    fn send<'a>(
        &'a self,
        _: &'a personal_ai_storage::replies::ReplyContext,
        _: &'a personal_ai_storage::reply_budgets::ReplyBudget,
    ) -> BoxFuture<'a, Result<ReplyCompletion, ReplySendError>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ReplyCompletion {
                content: "本地付费流程夹具".into(),
                usage: Some(ReplyUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                }),
            })
        })
    }
}
struct Fixture {
    state: AppState,
    store: Arc<PostgresStore>,
    pool: sqlx::PgPool,
    user: User,
    conversation: String,
    cookie: String,
    configuration: ReplyConfiguration,
    sender: Arc<Sender>,
}
impl Fixture {
    #[allow(clippy::too_many_lines)] // 组装真实数据库及固定供应商夹具。
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let store = Arc::new(PostgresStore::connect(&url).await.unwrap());
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let user = User {
            id: UserId::new(Uuid::new_v4().to_string()),
            email: format!("{}@paid.example", Uuid::new_v4()),
            display_name: "金额流程验收".into(),
        };
        store.save_user(&user).await.unwrap();
        store.set_password(&user.id, "fixture-hash").await.unwrap();
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        store
            .create_session(
                &user.id,
                &format!("{:x}", Sha256::digest(token.as_bytes())),
                "fixture-hash",
            )
            .await
            .unwrap();
        let conversation = store
            .create_conversation(&user.id, &Uuid::new_v4().to_string(), "金额确认")
            .await
            .unwrap()
            .id;
        store
            .append_message(&user.id, &conversation, &Uuid::new_v4().to_string(), "问题")
            .await
            .unwrap();
        let configuration = ReplyConfiguration {
            model: REPLY_MODEL.into(),
            revision: Uuid::new_v4().to_string(),
        };
        let policy = Arc::new(
            OpenAiReplyPolicy::new(
                configuration.clone(),
                ReplyPrices {
                    version: "fixture-only".into(),
                    input_per_million: 1_000_000,
                    output_per_million: 1_000_000,
                    request_limit: 129_024,
                    daily_limit: 258_048,
                },
            )
            .unwrap(),
        );
        let budget = policy
            .plan(&personal_ai_storage::replies::ReplyContext {
                system: "fixture".into(),
                user_messages: vec!["问题".into()],
                first_sequence: 1,
                max_output_tokens: 1024,
                configuration: configuration.clone(),
            })
            .unwrap();
        store
            .register_reply_configuration(&configuration, &budget, 4_102_444_800_000)
            .await
            .unwrap();
        let sender = Arc::new(Sender(AtomicUsize::new(0)));
        let paid = PaidReplies::new(
            store.clone(),
            configuration.clone(),
            budget,
            policy,
            sender.clone(),
        )
        .unwrap();
        let replies = Arc::new(ReplyRuntime {
            store: store.clone(),
            enabled: true,
            budget_store: Some(store.clone()),
            paid: Some(paid),
        });
        let state = AppState {
            learning: None,
            subscription_connections: None,
            feed_values: None,
            feeds: None,
            schedules: Some(store.clone()),
            model_agents: None,
            agent_plans: Some(store.clone()),
            web_search: None,
            git_tool: None,
            tool_calls: Some(store.clone()),
            replies: Some(replies),
            messages: store.clone(),
            message_cache: None,
            conversations: store.clone(),
            memories: store.clone(),
            indexing: None,
            answering: None,
            web_importer: Arc::new(personal_ai_web_import::PublicWebImporter::default()),
            documents: store.clone(),
            store: store.clone(),
            api_token: Arc::from("test-admin-token-01234567890123456789"),
            auth: Arc::new(AuthConfig::new(false)),
        };
        Self {
            state,
            store,
            pool,
            user,
            conversation,
            cookie: format!("personal_ai_session_v2={token}"),
            configuration,
            sender,
        }
    }
    fn path(&self) -> String {
        format!("/api/conversations/{}/replies", self.conversation)
    }
    fn input(&self, id: &str) -> serde_json::Value {
        json!({"request_id":id,"expected_revision":1,"configuration_revision":self.configuration.revision,"accepted_max_micro":"129024"})
    }
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: serde_json::Value,
        cookie: Option<&str>,
        csrf: bool,
    ) -> (StatusCode, serde_json::Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        if csrf {
            request = request.header("x-requested-with", "personal-ai");
        }
        let response = router(self.state.clone())
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        (status, value)
    }
    async fn post(&self, input: serde_json::Value) -> (StatusCode, serde_json::Value) {
        self.call("POST", &self.path(), input, Some(&self.cookie), true)
            .await
    }
    async fn occupied(&self) -> i64 {
        sqlx::query_scalar(
            "SELECT COALESCE(sum(occupied),0)::bigint FROM reply_money_daily WHERE user_id=$1",
        )
        .bind(Uuid::parse_str(self.user.id.as_str()).unwrap())
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
    async fn cleanup(self) {
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(Uuid::parse_str(self.user.id.as_str()).unwrap())
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM reply_configurations WHERE revision=$1")
            .bind(self.configuration.revision)
            .execute(&self.pool)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn paid_http_requires_consent_and_preserves_money_history_after_mode_change() {
    let mut f = Fixture::new().await;
    let request = Uuid::new_v4().to_string();
    let (status, history) = f
        .call("GET", &f.path(), json!(null), Some(&f.cookie), true)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history["quote"]["reservation_micro"], "129024");
    assert_eq!(history["mode"], "openai");
    for input in [
        json!({"request_id":request,"expected_revision":1}),
        json!({"request_id":request,"expected_revision":1,"configuration_revision":"old","accepted_max_micro":"129024"}),
        json!({"request_id":request,"expected_revision":1,"configuration_revision":f.configuration.revision,"accepted_max_micro":"129023"}),
    ] {
        assert_eq!(f.post(input).await.0, StatusCode::CONFLICT);
    }
    assert_eq!(f.occupied().await, 0);
    let input = f.input(&request);
    let (status, reply) = f.post(input.clone()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(reply["billing"]["reserved_micro"], "129024");
    assert!(!reply.to_string().contains("fixture-only"));
    assert_eq!(f.occupied().await, 129_024);
    f.state.replies.as_ref().unwrap().tick().await;
    assert_eq!(f.sender.0.load(Ordering::SeqCst), 1);
    assert_eq!(f.occupied().await, 30);
    f.store
        .disable_reply_configuration(&f.configuration.revision)
        .await
        .unwrap();
    let (_, history) = f
        .call("GET", &f.path(), json!(null), Some(&f.cookie), true)
        .await;
    assert_eq!(history["enabled"], false);
    assert_eq!(history["items"][0]["billing"]["charged_micro"], "30");
    // 配置关闭后，同 ID 仍返回已结算结果。新 ID 被事务检查拒绝。
    assert_eq!(f.post(input.clone()).await.0, StatusCode::ACCEPTED);
    assert_eq!(
        f.post(f.input(&Uuid::new_v4().to_string())).await.0,
        StatusCode::CONFLICT
    );
    f.state.replies = Some(Arc::new(ReplyRuntime {
        store: f.store.clone(),
        enabled: false,
        budget_store: Some(f.store.clone()),
        paid: None,
    }));
    let (status, reply) = f.post(input).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(reply["mode"], "openai");
    assert_eq!(reply["billing"]["charged_micro"], "30");
    assert_eq!(f.sender.0.load(Ordering::SeqCst), 1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn paid_http_enforces_identity_csrf_quota_and_cancel_refund() {
    let f = Fixture::new().await;
    let foreign = Fixture::new().await;
    let request = Uuid::new_v4().to_string();
    let input = f.input(&request);
    assert_eq!(
        f.call("POST", &f.path(), input.clone(), None, true).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.call("POST", &f.path(), input.clone(), Some(&f.cookie), false)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.call(
            "POST",
            &f.path(),
            input.clone(),
            Some(&foreign.cookie),
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let mut unknown = input.clone();
    unknown["input_price"] = json!(1);
    assert_eq!(f.post(unknown).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(f.occupied().await, 0);
    assert_eq!(f.post(input).await.0, StatusCode::ACCEPTED);
    let detail = format!("{}/{}", f.path(), request);
    assert_eq!(
        f.call("GET", &detail, json!(null), Some(&foreign.cookie), true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.call("GET", &f.path(), json!(null), Some(&foreign.cookie), true)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.call(
            "POST",
            &format!("{detail}/cancel"),
            json!(null),
            Some(&f.cookie),
            false
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, cancelled) = f
        .call(
            "POST",
            &format!("{detail}/cancel"),
            json!(null),
            Some(&f.cookie),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cancelled["billing"]["charged_micro"], "0");
    assert_eq!(f.occupied().await, 0);
    // 最后一份日金额不足，次数和金额同事务回滚。
    sqlx::query("UPDATE reply_money_daily SET occupied=129025 WHERE user_id=$1")
        .bind(Uuid::parse_str(f.user.id.as_str()).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.post(f.input(&Uuid::new_v4().to_string())).await.0,
        StatusCode::CONFLICT
    );
    let count: i32 =
        sqlx::query_scalar("SELECT reserved FROM reply_daily_budgets WHERE user_id=$1")
            .bind(Uuid::parse_str(f.user.id.as_str()).unwrap())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    assert_eq!(f.sender.0.load(Ordering::SeqCst), 0);
    f.cleanup().await;
    foreign.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn expired_configuration_cannot_reserve_or_send_and_legacy_replay_stays_free() {
    let f = Fixture::new().await;
    let legacy = Uuid::new_v4().to_string();
    f.store
        .reserve_reply(&f.user.id, &f.conversation, &legacy, 1, &configuration())
        .await
        .unwrap();
    assert_eq!(f.post(f.input(&legacy)).await.1["mode"], "fixture");
    assert_eq!(f.occupied().await, 0);
    f.store
        .cancel_reply(&f.user.id, &f.conversation, &legacy)
        .await
        .unwrap();
    sqlx::query("UPDATE reply_configurations SET valid_until_ms=1 WHERE revision=$1")
        .bind(&f.configuration.revision)
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.post(f.input(&Uuid::new_v4().to_string())).await.0,
        StatusCode::CONFLICT
    );
    let (_, history) = f
        .call("GET", &f.path(), json!(null), Some(&f.cookie), true)
        .await;
    assert_eq!(history["enabled"], false);
    f.state.replies.as_ref().unwrap().tick().await;
    assert_eq!(f.sender.0.load(Ordering::SeqCst), 0);
    f.cleanup().await;
}

#[path = "model_agent_tests.rs"]
mod model_agents;

#[path = "schedule_tests.rs"]
mod schedules;
