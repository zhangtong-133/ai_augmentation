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
            Ok(r#"{"items":[{"id":1,"score":null,"reason":"摘要为空，无法评分。"}]}"#.into())
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
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn local_value_operations_reports_unknown_without_recovery_and_detects_missing_send_audit() {
    use personal_ai_storage::feed_value_operations::FeedValueOperationsStore;
    let f = fixture().await;
    let draft = draft(&f).await;
    let authorized = authorize(&f).await;
    let claim = f
        .store
        .claim_local_value(&f.owner, &authorized.request_id)
        .await
        .unwrap()
        .unwrap();
    assert!(f.store.begin_local_value(&claim, &target()).await.unwrap());
    f.store.finish_local_value(&claim, None).await.unwrap();
    let report = f.store.audit_feed_values(&f.owner, None).await.unwrap();
    assert!(report.consistent);
    assert_eq!(report.counts["records"], 2);
    assert_eq!(report.counts["sent"], 1);
    assert_eq!(report.counts["unknown"], 1);
    assert_eq!(report.remaining_previews_today, 18);
    assert_eq!(report.warnings, ["unknown"]);
    let encoded = serde_json::to_string(&report).unwrap();
    for field in [
        "qwen3",
        "127.0.0.1",
        "keywords",
        "\"snapshot\":",
        "scores",
        "digest",
        "dispatch_token",
    ] {
        assert!(!encoded.contains(field));
    }
    let other = f.store.audit_feed_values(&f.other, None).await.unwrap();
    assert!(other.items.is_empty());
    assert_eq!(other.counts["records"], 0);
    assert!(
        f.store
            .audit_feed_values(&f.owner, Some("bad"))
            .await
            .is_err()
    );
    sqlx::query(
        "DELETE FROM feed_value_audit WHERE user_id=$1 AND request_id=$2 AND event='sending'",
    )
    .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
    .bind(Uuid::parse_str(&authorized.request_id).unwrap())
    .execute(&f.pool)
    .await
    .unwrap();
    let bad = f.store.audit_feed_values(&f.owner, None).await.unwrap();
    assert!(!bad.consistent);
    assert_eq!(bad.counts["inconsistent_records"], 1);
    assert!(
        bad.items
            .iter()
            .find(|i| i.request_id == authorized.request_id)
            .unwrap()
            .issues
            .contains(&"sending_audit_mismatch".into())
    );
    assert_eq!(
        f.store
            .get_feed_value(&f.owner, &authorized.request_id)
            .await
            .unwrap()
            .status,
        "unknown"
    );
    assert_eq!(
        f.store
            .get_feed_value(&f.owner, &draft.request_id)
            .await
            .unwrap()
            .status,
        "draft"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn local_value_operations_pages_metadata_with_global_counts_and_no_cleanup() {
    use personal_ai_storage::feed_value_operations::FeedValueOperationsStore;
    let f = fixture().await;
    let saved = draft(&f).await;
    let owner = Uuid::parse_str(f.owner.as_str()).unwrap();
    // Old synthetic records deliberately retain their opaque payloads: the audit must not parse them.
    sqlx::query("INSERT INTO feed_value_reviews(user_id,id,status,snapshot,pricing,digest,created_ms,expires_ms) SELECT user_id,gen_random_uuid(),'draft',snapshot,pricing,digest,created_ms-86400000,expires_ms-86400000 FROM feed_value_reviews CROSS JOIN generate_series(1,100) WHERE user_id=$1 AND id=$2")
        .bind(owner).bind(Uuid::parse_str(&saved.request_id).unwrap()).execute(&f.pool).await.unwrap();
    let first = f.store.audit_feed_values(&f.owner, None).await.unwrap();
    assert!(first.consistent);
    assert_eq!(first.items.len(), 100);
    assert_eq!(first.counts["records"], 101);
    assert_eq!(first.counts["previews_today"], 1);
    assert_eq!(first.counts["expired_active"], 100);
    assert_eq!(first.remaining_previews_today, 19);
    let second = f
        .store
        .audit_feed_values(&f.owner, first.next_cursor.as_deref())
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(second.next_cursor.is_none());
    assert_eq!(first.counts, second.counts);
    assert!(
        first
            .items
            .iter()
            .all(|i| i.request_id != second.items[0].request_id)
    );
    assert_eq!(second.remaining_records, 899);
    assert!(matches!(
        f.store
            .audit_feed_values(&UserId::new(Uuid::new_v4().to_string()), None)
            .await,
        Err(StorageError::NotFound)
    ));
    f.cleanup().await;
}

// Reconstruct an already-issued v1 consent without allowing new v1 previews.
async fn legacy(f: &Fixture, saved: &ValueReview) -> ValueReview {
    use sha2::{Digest, Sha256};
    let snapshot = saved.snapshot.as_ref().unwrap();
    let mut pricing = saved.pricing.clone();
    if let ValuePricing::Local { profile, .. } = &mut pricing {
        *profile = "local-rss-v1".into();
    }
    let plan = personal_ai_agent_core::feed_value::plan_value_scoring(
        &f.owner,
        &saved.request_id,
        snapshot.day_start_unix_ms,
        snapshot.as_of_unix_ms,
        &snapshot.keywords,
        &snapshot.candidates,
    )
    .unwrap()
    .unwrap();
    let encoded = serde_json::to_vec(&(
        "rss-value-review-v1",
        plan.digest(),
        snapshot.preference_revision,
        &pricing,
        saved.expires_at_unix_ms,
    ))
    .unwrap();
    let digest = format!("{:x}", Sha256::digest(encoded));
    assert_ne!(digest, saved.digest);
    sqlx::query("UPDATE feed_value_reviews SET pricing=$3,digest=$4 WHERE user_id=$1 AND id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .bind(Uuid::parse_str(&saved.request_id).unwrap())
        .bind(serde_json::to_value(pricing).unwrap())
        .bind(digest)
        .execute(&f.pool)
        .await
        .unwrap();
    f.store
        .get_feed_value(&f.owner, &saved.request_id)
        .await
        .unwrap()
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn local_profile_upgrade_keeps_legacy_readable_but_blocks_old_approval_claim_and_send() {
    let f = fixture().await;
    let current = draft(&f).await;
    assert!(
        matches!(&current.pricing, ValuePricing::Local {profile,..} if profile == "local-rss-v2")
    );
    let old_draft = legacy(&f, &current).await;
    is_conflict(
        f.store
            .approve_feed_value(&f.owner, &old_draft.request_id, &consent(&old_draft))
            .await,
    );
    let old_authorized = legacy(&f, &authorize(&f).await).await;
    assert!(
        f.store
            .claim_local_value(&f.owner, &old_authorized.request_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(sent(&f, &old_authorized.request_id).await, 0);
    for already_sent in [false, true] {
        let saved = authorize(&f).await;
        let mut claim = f
            .store
            .claim_local_value(&f.owner, &saved.request_id)
            .await
            .unwrap()
            .unwrap();
        if already_sent {
            assert!(f.store.begin_local_value(&claim, &target()).await.unwrap());
        }
        claim.review = legacy(&f, &claim.review).await;
        assert!(!f.store.begin_local_value(&claim, &target()).await.unwrap());
        let output = already_sent.then(|| {
            br#"{"items":[{"id":1,"score":75,"reason":"legacy freeform reason"}]}"#.to_vec()
        });
        let finished = f.store.finish_local_value(&claim, output).await.unwrap();
        assert_eq!(
            finished.status,
            if already_sent { "succeeded" } else { "unknown" }
        );
        let read = f
            .store
            .get_feed_value(&f.owner, &saved.request_id)
            .await
            .unwrap();
        assert_eq!(read.scores.is_some(), already_sent);
        assert!(
            f.store
                .claim_local_value(&f.owner, &saved.request_id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(sent(&f, &saved.request_id).await, i64::from(already_sent));
    }
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn local_v2_rejects_freeform_reasons_and_non_abstention_without_persisting_or_resending() {
    let f = fixture().await;
    sqlx::query("UPDATE feed_entries SET summary='Rust ownership tutorial' WHERE user_id=$1")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    for empty in [false, true] {
        if empty {
            sqlx::query("UPDATE feed_entries SET summary='' WHERE user_id=$1")
                .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        let saved = authorize(&f).await;
        let claim = f
            .store
            .claim_local_value(&f.owner, &saved.request_id)
            .await
            .unwrap()
            .unwrap();
        assert!(f.store.begin_local_value(&claim, &target()).await.unwrap());
        let reason = if empty {
            "摘要为空，无法评分。"
        } else {
            "copied instruction"
        };
        let output = serde_json::to_vec(
            &serde_json::json!({"items":[{"id":1,"score":100,"reason":reason}]}),
        )
        .unwrap();
        let result = f
            .store
            .finish_local_value(&claim, Some(output))
            .await
            .unwrap();
        assert_eq!(result.status, "unknown");
        assert!(result.scores.is_none());
        assert!(result.snapshot.is_none());
        assert!(
            f.store
                .claim_local_value(&f.owner, &saved.request_id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(sent(&f, &saved.request_id).await, 1);
    }
    f.cleanup().await;
}
