use super::*;
use personal_ai_agent_core::feed_value_local::{LOCAL_VALUE_PROFILE, execute_local_value};
use personal_ai_llm::{
    ChatRequest, LlmResult,
    local::{LocalInference, LocalTarget},
    stream::TextDeltaSink,
};
use personal_ai_storage::{
    BoxFuture,
    feed_value::{FeedValueExecutionStore, LocalValueExecutionStore},
};
use sqlx::Row;

struct LocalPlanner {
    endpoint: String,
}
impl ValueQuotePlanner for LocalPlanner {
    fn quote(&self, _: &UserId, _: &str, snapshot: &ValueSnapshot) -> StorageResult<ValuePricing> {
        Ok(ValuePricing::Local {
            endpoint: self.endpoint.clone(),
            model: "qwen3:4b-q4_K_M".into(),
            profile: LOCAL_VALUE_PROFILE.into(),
            valid_until_unix_ms: i64::try_from(snapshot.as_of_unix_ms).unwrap() + 300_000,
        })
    }
}
fn target() -> LocalTarget {
    LocalTarget::new("http://127.0.0.1:11435", "qwen3:4b-q4_K_M").unwrap()
}
async fn fixture() -> Fixture {
    let f = Fixture::new().await;
    let sub = f.sub().await;
    let claim = f.claim(&sub).await;
    f.store
        .finish_collection(&claim, response(ITEM))
        .await
        .unwrap();
    f.store
        .save_brief_preferences(&f.owner, 0, &["Rust".into()])
        .await
        .unwrap();
    f
}
async fn draft(f: &Fixture) -> ValueReview {
    f.store
        .preview_feed_value(
            &f.owner,
            &Uuid::new_v4().to_string(),
            Arc::new(LocalPlanner {
                endpoint: target().endpoint().into(),
            }),
        )
        .await
        .unwrap()
}
fn consent(saved: &ValueReview) -> ValueApproval {
    let mut approval = approval(saved);
    approval.acknowledge_local_compute = true;
    approval
}
async fn authorize(f: &Fixture) -> ValueReview {
    let saved = draft(f).await;
    f.store
        .approve_feed_value(&f.owner, &saved.request_id, &consent(&saved))
        .await
        .unwrap()
}
async fn sent(f: &Fixture, request: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM feed_value_audit WHERE user_id=$1 AND request_id=$2 AND event='sending'").bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(request).unwrap()).fetch_one(&f.pool).await.unwrap()
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn feed_value_local_consent_is_distinct_private_exact_and_bounded() {
    let f = fixture().await;
    let saved = draft(&f).await;
    assert_eq!(saved.amount, None);
    let again = f
        .store
        .preview_feed_value(
            &f.owner,
            &saved.request_id,
            Arc::new(LocalPlanner {
                endpoint: "http://127.0.0.1:22222".into(),
            }),
        )
        .await
        .unwrap();
    assert!(again == saved);
    for n in 0..4 {
        let mut wrong = consent(&saved);
        match n {
            0 => wrong.acknowledge_local_compute = false,
            1 => wrong.acknowledge_subscription_usage = true,
            2 => wrong.acknowledge_cost = true,
            _ => wrong.digest = "0".repeat(64),
        }
        is_conflict(
            f.store
                .approve_feed_value(&f.owner, &saved.request_id, &wrong)
                .await,
        );
    }
    assert!(matches!(
        f.store
            .approve_feed_value(&f.other, &saved.request_id, &consent(&saved))
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(
        f.store
            .preview_feed_value(
                &f.owner,
                &Uuid::new_v4().to_string(),
                Arc::new(LocalPlanner {
                    endpoint: "http://192.168.1.1:11435".into()
                })
            )
            .await
            .is_err()
    );
    let approved = f
        .store
        .approve_feed_value(&f.owner, &saved.request_id, &consent(&saved))
        .await
        .unwrap();
    assert_eq!(approved.status, "authorized");
    for _ in 1..20 {
        draft(&f).await;
    }
    is_conflict(
        f.store
            .preview_feed_value(
                &f.owner,
                &Uuid::new_v4().to_string(),
                Arc::new(LocalPlanner {
                    endpoint: target().endpoint().into(),
                }),
            )
            .await,
    );
    for table in ["subscription_connections", "reply_money_reservations"] {
        let count: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE user_id=$1"))
                .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn feed_value_local_claim_is_single_and_unknown_outputs_never_resend() {
    let f = fixture().await;
    let saved = authorize(&f).await;
    assert!(
        f.store
            .claim_subscription_value(&f.owner, &saved.request_id)
            .await
            .unwrap()
            .is_none()
    );
    let (a, b) = tokio::join!(
        f.store.claim_local_value(&f.owner, &saved.request_id),
        f.store.claim_local_value(&f.owner, &saved.request_id)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.is_some(), b.is_some());
    let claim = a.or(b).unwrap();
    let wrong = LocalTarget::new("http://127.0.0.1:22222", target().model()).unwrap();
    is_conflict(f.store.begin_local_value(&claim, &wrong).await);
    assert_eq!(sent(&f, &saved.request_id).await, 0);
    assert!(f.store.begin_local_value(&claim, &target()).await.unwrap());
    assert!(!f.store.begin_local_value(&claim, &target()).await.unwrap());
    assert!(
        f.store
            .finish_subscription_value(&claim, None)
            .await
            .is_err()
    );
    let result = f
        .store
        .finish_local_value(&claim, Some(br#"{"items":[]}"#.to_vec()))
        .await
        .unwrap();
    assert_eq!(result.status, "unknown");
    assert!(result.snapshot.is_none());
    assert!(result.scores.is_none());
    assert!(
        f.store
            .claim_local_value(&f.owner, &saved.request_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(sent(&f, &saved.request_id).await, 1);
    f.cleanup().await;
}
struct Runtime<'a> {
    calls: AtomicUsize,
    store: &'a PostgresStore,
    owner: &'a UserId,
    request: String,
    cancel: bool,
}
impl LocalInference for Runtime<'_> {
    fn infer<'a>(
        &'a self,
        _: &'a LocalTarget,
        chat: &'a ChatRequest,
        _: &'a dyn TextDeltaSink,
    ) -> BoxFuture<'a, LlmResult<String>> {
        Box::pin(async move {
            assert_eq!(chat.max_output_tokens, Some(2048));
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.cancel {
                self.store
                    .cancel_feed_value(self.owner, &self.request)
                    .await
                    .unwrap();
            }
            Ok(r#"{"items":[{"id":1,"score":null,"reason":"insufficient evidence"}]}"#.into())
        })
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn feed_value_local_execution_persists_only_valid_current_output_and_discards_cancelled_late_results()
 {
    let f = fixture().await;
    for cancel in [false, true] {
        let saved = authorize(&f).await;
        let runtime = Runtime {
            calls: AtomicUsize::new(0),
            store: &f.store,
            owner: &f.owner,
            request: saved.request_id.clone(),
            cancel,
        };
        let result =
            execute_local_value(&f.store, &runtime, &target(), &f.owner, &saved.request_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            result.status,
            if cancel { "cancelled" } else { "succeeded" }
        );
        assert_eq!(result.scores.is_some(), !cancel);
        assert!(
            execute_local_value(&f.store, &runtime, &target(), &f.owner, &saved.request_id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
        assert_eq!(sent(&f, &saved.request_id).await, 1);
    }
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn feed_value_local_source_revocation_and_deadline_clear_data_and_prevent_send() {
    let f = fixture().await;
    let saved = authorize(&f).await;
    let claim = f
        .store
        .claim_local_value(&f.owner, &saved.request_id)
        .await
        .unwrap()
        .unwrap();
    f.store
        .save_brief_preferences(&f.owner, 1, &["storage".into()])
        .await
        .unwrap();
    assert!(!f.store.begin_local_value(&claim, &target()).await.unwrap());
    let invalid = f.store.finish_local_value(&claim, None).await.unwrap();
    assert_eq!(invalid.status, "invalidated");
    assert!(invalid.snapshot.is_none());
    let saved = authorize(&f).await;
    let _claim = f
        .store
        .claim_local_value(&f.owner, &saved.request_id)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE feed_value_reviews SET dispatch_deadline_ms=approved_ms+1 WHERE user_id=$1 AND id=$2").bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&saved.request_id).unwrap()).execute(&f.pool).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let unknown = f
        .store
        .get_feed_value(&f.owner, &saved.request_id)
        .await
        .unwrap();
    assert_eq!(unknown.status, "unknown");
    assert!(unknown.snapshot.is_none());
    assert!(
        f.store
            .claim_local_value(&f.owner, &saved.request_id)
            .await
            .unwrap()
            .is_none()
    );
    let row = sqlx::query("SELECT sent_ms FROM feed_value_reviews WHERE user_id=$1 AND id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .bind(Uuid::parse_str(&saved.request_id).unwrap())
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(row.get::<Option<i64>, _>("sent_ms"), None);
    f.cleanup().await;
}
