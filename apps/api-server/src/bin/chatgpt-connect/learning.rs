//! Explicit local execution of an existing learning authorization; no automatic approval.
use super::{store::Store, value_runtime::Runtime};
use personal_ai_agent_core::{
    feed_value_execution::SubscriptionValueRuntime,
    learning_model_execution::{
        ReviewRuntimeError, SubscriptionReviewRuntime, execute_model_review, observe_model_review,
        text_bridge::relay_progress,
    },
};
use personal_ai_domain::UserId;
use personal_ai_llm::ChatRequest;
use personal_ai_llm_openai::chatgpt::{ChatGptClient, Error, Result};
use personal_ai_storage::{
    BoxFuture,
    feed_value::ValuePricing,
    learning::review_text::ReviewTextBridge,
    learning::{LearningStore, model_authorization::ModelAuthorization},
    subscription_connections::VerifiedSubscriptionConnection,
};
use personal_ai_storage_postgres::PostgresStore;
fn pricing(item: &ModelAuthorization) -> ValuePricing {
    ValuePricing::Subscription {
        provider: "chatgpt-plan".into(),
        model: item.model.clone(),
        configuration_version: format!("connection-v{}", item.connection_revision),
        connection_id: item.connection_id.clone(),
        valid_until_unix_ms: i64::try_from(item.expires_at_unix_ms).unwrap_or(i64::MAX),
    }
}
impl SubscriptionReviewRuntime for Runtime<'_> {
    fn review_observed<'a>(
        &'a self,
        item: &'a ModelAuthorization,
        request: &'a ChatRequest,
        sink: &'a dyn personal_ai_llm::stream::TextDeltaSink,
    ) -> BoxFuture<'a, std::result::Result<Vec<u8>, ReviewRuntimeError>> {
        Box::pin(async move {
            let registration = self
                .store
                .data
                .accounts
                .get(self.label)
                .ok_or(ReviewRuntimeError)?;
            self.client
                .score_observed(registration, &item.model, request, sink)
                .await
                .map(String::into_bytes)
                .map_err(|_| ReviewRuntimeError)
        })
    }

    fn verify<'a>(
        &'a self,
        item: &'a ModelAuthorization,
    ) -> BoxFuture<'a, std::result::Result<VerifiedSubscriptionConnection, ReviewRuntimeError>>
    {
        Box::pin(async move {
            SubscriptionValueRuntime::verify(self, &pricing(item))
                .await
                .map_err(|_| ReviewRuntimeError)
        })
    }
    fn review<'a>(
        &'a self,
        item: &'a ModelAuthorization,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, std::result::Result<Vec<u8>, ReviewRuntimeError>> {
        Box::pin(async move {
            SubscriptionValueRuntime::score(self, &pricing(item), request)
                .await
                .map_err(|_| ReviewRuntimeError)
        })
    }
}
fn uuid(value: &str) -> Result<String> {
    uuid::Uuid::parse_str(value)
        .ok()
        .filter(|id| !id.is_nil())
        .map(|id| id.to_string())
        .ok_or(Error("invalid learning UUID"))
}
fn parse(args: &[String]) -> Result<(UserId, String, Option<&str>)> {
    match args.get(1).map(String::as_str) {
        Some("learning-run") if args.len() == 6 && args[5] == "--use-subscription" => {
            super::store::check_label(&args[2])?;
            Ok((
                UserId::new(uuid(&args[3])?),
                uuid(&args[4])?,
                Some(&args[2]),
            ))
        }
        Some("learning-show") if args.len() == 4 => {
            Ok((UserId::new(uuid(&args[2])?), uuid(&args[3])?, None))
        }
        _ => Err(Error("invalid learning arguments; see --help")),
    }
}
fn text_bridge_from_env() -> Result<Option<personal_ai_storage_redis::RedisReviewText>> {
    match std::env::var("LEARNING_TEXT_REDIS_URL") {
        Ok(url) if url.is_empty() => Ok(None),
        Ok(url) => personal_ai_storage_redis::RedisReviewText::new(&url)
            .map(Some)
            .map_err(|_| Error("invalid learning text bridge configuration")),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(Error("invalid learning text bridge configuration")),
    }
}
pub(super) async fn run(args: &[String]) -> Result<()> {
    let (owner, request, label) = parse(args)?;
    let url = std::env::var("DATABASE_URL")
        .map_err(|_| Error("DATABASE_URL required for learning execution"))?;
    let db = PostgresStore::connect(&url)
        .await
        .map_err(|_| Error("learning database unavailable"))?;
    let failed = |_| {
        Error(
            "learning outcome not confirmed; inspect original request with learning-show; do not resend",
        )
    };
    let mut item = db
        .get_model_authorization(&owner, &request)
        .await
        .map_err(failed)?;
    if let Some(label) = label.filter(|_| item.status == "authorized") {
        let mut local = Store::open(std::path::Path::new(&args[0]))?;
        let client = ChatGptClient::new()?;
        let registration = local
            .data
            .accounts
            .get_mut(label)
            .ok_or(Error("unknown account label"))?;
        if !registration.plan_enabled() {
            return Err(Error("subscription permission required"));
        }
        if registration.needs_refresh()? {
            client.refresh(registration).await?;
            local.save()?;
        }
        let runtime = Runtime {
            client: &client,
            store: &local,
            label,
        };
        eprintln!(
            "核对原核验授权后单次发送；可能消耗订阅额度或账户允许的 credits，未知结果不会重发。"
        );
        let bridge = text_bridge_from_env()?;
        let result = if let Some(bridge) = bridge {
            if let Ok(mut publisher) = bridge.publisher(&owner, &request).await {
                let (receiver, execution) = observe_model_review(&db, &runtime, &owner, &request);
                let (result, ()) = tokio::join!(
                    execution,
                    relay_progress(receiver, publisher.as_mut(), &owner, &request)
                );
                result
            } else {
                eprintln!("临时文本通道不可用；继续原授权执行，完成后核对保存状态。");
                execute_model_review(&db, &runtime, &owner, &request).await
            }
        } else {
            execute_model_review(&db, &runtime, &owner, &request).await
        };
        item = match result.map_err(failed)? {
            Some(item) => item,
            None => db
                .get_model_authorization(&owner, &request)
                .await
                .map_err(failed)?,
        };
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&item).map_err(|_| Error("invalid learning output"))?
    );
    if label.is_some() && item.status != "succeeded" {
        return Err(Error(
            "learning review not completed; inspect status; no automatic retry",
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Sink(std::sync::Mutex<String>);
    impl personal_ai_llm::stream::TextDeltaSink for Sink {
        fn delta(&self, text: &str) {
            self.0.lock().unwrap().push_str(text);
        }
    }

    #[test]
    fn execution_requires_exact_explicit_flag_and_valid_ids() {
        let owner = uuid::Uuid::new_v4().to_string();
        let request = uuid::Uuid::new_v4().to_string();
        let args = vec![
            "/tmp/unused".into(),
            "learning-run".into(),
            "fixture".into(),
            owner,
            request,
            "--use-subscription".into(),
        ];
        assert!(parse(&args).is_ok());
        assert!(parse(&args[..5]).is_err());
        let mut bad = args.clone();
        bad[5] = "--use-api".into();
        assert!(parse(&bad).is_err());
        bad = args;
        bad[3] = uuid::Uuid::nil().to_string();
        assert!(parse(&bad).is_err());
    }
    #[tokio::test]
    async fn learning_runtime_reuses_the_locked_account_without_api_parameters() {
        use super::super::value_runtime::tests::{Client, local};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (path, store) = local();
        let client = Client {
            calls: AtomicUsize::new(0),
        };
        let runtime = Runtime {
            client: &client,
            store: &store,
            label: "fixture",
        };
        let item = ModelAuthorization {
            local_endpoint: None,
            request_id: uuid::Uuid::new_v4().to_string(),
            plan_id: uuid::Uuid::new_v4().to_string(),
            task_id: uuid::Uuid::new_v4().to_string(),
            connection_id: uuid::Uuid::new_v4().to_string(),
            connection_revision: 1,
            model: "fixture".into(),
            status: "running".into(),
            digest: "a".repeat(64),
            created_at_unix_ms: 1,
            expires_at_unix_ms: 300_001,
            approved_at_unix_ms: Some(2),
            preview: None,
            advice: None,
        };
        let proof = SubscriptionReviewRuntime::verify(&runtime, &item)
            .await
            .unwrap();
        assert_eq!(proof.subject, "fixture-user");
        assert!(Store::open(&path).is_err());
        let request = ChatRequest {
            messages: vec![
                personal_ai_llm::ChatMessage {
                    role: personal_ai_llm::Role::System,
                    content: "核验规则".into(),
                },
                personal_ai_llm::ChatMessage {
                    role: personal_ai_llm::Role::User,
                    content: "冻结材料".into(),
                },
            ],
            temperature: None,
            max_output_tokens: None,
        };
        let sink = Sink(std::sync::Mutex::new(String::new()));
        SubscriptionReviewRuntime::review_observed(&runtime, &item, &request, &sink)
            .await
            .unwrap();
        assert_eq!(*sink.0.lock().unwrap(), "provisional fixture text");
        assert_eq!(client.calls.load(Ordering::SeqCst), 1);
        drop(store);
        std::fs::remove_dir_all(path).unwrap();
    }
}
