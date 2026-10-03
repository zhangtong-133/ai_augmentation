use super::*;
use personal_ai_agent_core::feed_value_execution::{
    SubscriptionValueRuntime, ValueRuntimeError, execute_subscription_value,
};
use personal_ai_storage::{
    BoxFuture,
    subscription_connections::{SubscriptionConnectionStore, VerifiedSubscriptionConnection},
};
use sqlx::Row;

const OUTPUT: &[u8] = br#"{"items":[{"id":1,"score":null,"reason":"insufficient evidence"}]}"#;
async fn authorized(f: &Fixture, subscription: bool) -> ValueReview {
    let saved = preview(f, planner(subscription)).await;
    f.store
        .approve_feed_value(&f.owner, &saved.request_id, &approval(&saved))
        .await
        .unwrap()
}
async fn proof(f: &Fixture) -> VerifiedSubscriptionConnection {
    let row = sqlx::query(
        "SELECT host_id,client_id,expires_ms FROM subscription_connections WHERE user_id=$1",
    )
    .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    VerifiedSubscriptionConnection {
        host_id: row.get::<Uuid, _>("host_id").urn().to_string(),
        client_id: row.get("client_id"),
        subject: "fixture".into(),
        label: "fixture".into(),
        models: vec!["fixture".into()],
        valid_until_unix_ms: row.get("expires_ms"),
    }
}
async fn claim(f: &Fixture) -> ValueClaim {
    let saved = authorized(f, true).await;
    f.store
        .claim_subscription_value(&f.owner, &saved.request_id)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn value_dispatch_claims_once_and_never_claims_api_or_foreign_reviews() {
    let (f, _) = setup().await;
    let draft = preview(&f, planner(true)).await;
    assert!(
        f.store
            .claim_subscription_value(&f.owner, &draft.request_id)
            .await
            .unwrap()
            .is_none()
    );
    let api = authorized(&f, false).await;
    assert!(
        f.store
            .claim_subscription_value(&f.owner, &api.request_id)
            .await
            .unwrap()
            .is_none()
    );
    let saved = authorized(&f, true).await;
    assert!(matches!(
        f.store
            .claim_subscription_value(&f.other, &saved.request_id)
            .await,
        Err(StorageError::NotFound)
    ));
    let (a, b) = tokio::join!(
        f.store
            .claim_subscription_value(&f.owner, &saved.request_id),
        f.store
            .claim_subscription_value(&f.owner, &saved.request_id)
    );
    let claims = [a.unwrap(), b.unwrap()];
    assert_eq!(claims.iter().filter(|c| c.is_some()).count(), 1);
    let mut claim = claims.into_iter().flatten().next().unwrap();
    let original = proof(&f).await;
    let token = claim.token.clone();
    claim.token = Uuid::new_v4().to_string();
    is_conflict(f.store.begin_subscription_value(&claim, &original).await);
    claim.token = token;
    let review = claim.review.clone();
    claim.review.snapshot.as_mut().unwrap().keywords = vec!["changed".into()];
    is_conflict(f.store.begin_subscription_value(&claim, &original).await);
    claim.review = review;

    for field in 0..5 {
        let mut wrong = original.clone();
        match field {
            0 => wrong.host_id = Uuid::new_v4().urn().to_string(),
            1 => wrong.client_id = "oaiapp_other".into(),
            2 => wrong.subject = "other".into(),
            3 => wrong.models = vec!["other".into()],
            _ => wrong.valid_until_unix_ms = 1,
        }
        is_conflict(f.store.begin_subscription_value(&claim, &wrong).await);
    }
    assert!(
        f.store
            .begin_subscription_value(&claim, &original)
            .await
            .unwrap()
    );
    assert!(
        !f.store
            .begin_subscription_value(&claim, &original)
            .await
            .unwrap()
    );
    let saved = f
        .store
        .finish_subscription_value(&claim, Some(OUTPUT.to_vec()))
        .await
        .unwrap();
    assert_eq!(saved.status, "succeeded");
    assert_eq!(saved.scores.as_ref().unwrap().len(), 1);
    assert!(saved.scores.as_ref().unwrap()[0].score.is_none());
    assert!(
        f.store
            .finish_subscription_value(&claim, None)
            .await
            .unwrap()
            == saved
    );
    let reconnected = PostgresStore::connect_existing(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert!(
        reconnected
            .get_feed_value(&f.owner, &claim.request_id)
            .await
            .unwrap()
            == saved
    );
    assert!(
        reconnected
            .claim_subscription_value(&f.owner, &claim.request_id)
            .await
            .unwrap()
            .is_none()
    );
    let audit = f
        .store
        .feed_value_audit(&f.owner, &claim.request_id)
        .await
        .unwrap();
    assert_eq!(
        audit.iter().map(|a| a.event.as_str()).collect::<Vec<_>>(),
        vec!["draft", "authorized", "running", "sending", "succeeded"]
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn value_dispatch_cancellation_and_revocation_discard_late_results() {
    let (f, _) = setup().await;
    let proof = proof(&f).await;
    let before = claim(&f).await;
    f.store
        .cancel_feed_value(&f.owner, &before.request_id)
        .await
        .unwrap();
    assert!(
        !f.store
            .begin_subscription_value(&before, &proof)
            .await
            .unwrap()
    );
    assert_eq!(
        f.store
            .finish_subscription_value(&before, Some(OUTPUT.to_vec()))
            .await
            .unwrap()
            .status,
        "cancelled"
    );
    let after = claim(&f).await;
    assert!(
        f.store
            .begin_subscription_value(&after, &proof)
            .await
            .unwrap()
    );
    f.store
        .cancel_feed_value(&f.owner, &after.request_id)
        .await
        .unwrap();
    let saved = f
        .store
        .finish_subscription_value(&after, Some(OUTPUT.to_vec()))
        .await
        .unwrap();
    assert_eq!(saved.status, "cancelled");
    assert!(saved.snapshot.is_none() && saved.scores.is_none());
    let revoked = claim(&f).await;
    assert!(
        f.store
            .begin_subscription_value(&revoked, &proof)
            .await
            .unwrap()
    );
    f.store
        .revoke_subscription_connection(&f.owner, f.owner.as_str(), 1)
        .await
        .unwrap();
    let saved = f
        .store
        .finish_subscription_value(&revoked, Some(OUTPUT.to_vec()))
        .await
        .unwrap();
    assert_eq!(saved.status, "invalidated");
    assert!(saved.snapshot.is_none() && saved.scores.is_none());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn value_dispatch_unknown_results_and_deadlines_never_restore_dispatch() {
    let (f, _) = setup().await;
    let proof = proof(&f).await;
    for output in [
        None,
        Some(b"provider secret raw error".to_vec()),
        Some(br#"{"items":[]}"#.to_vec()),
    ] {
        let claim = claim(&f).await;
        assert!(
            f.store
                .begin_subscription_value(&claim, &proof)
                .await
                .unwrap()
        );
        let saved = f
            .store
            .finish_subscription_value(&claim, output)
            .await
            .unwrap();
        assert_eq!(saved.status, "unknown");
        assert!(saved.snapshot.is_none() && saved.scores.is_none());
        assert!(
            f.store
                .claim_subscription_value(&f.owner, &claim.request_id)
                .await
                .unwrap()
                .is_none()
        );
    }
    // A process can disappear before or after the send marker. Neither case may be dispatched again.
    for sent in [false, true] {
        let claim = claim(&f).await;
        if sent {
            assert!(
                f.store
                    .begin_subscription_value(&claim, &proof)
                    .await
                    .unwrap()
            );
        }
        sqlx::query("UPDATE feed_value_reviews SET dispatch_deadline_ms=approved_ms+1,sent_ms=CASE WHEN sent_ms IS NULL THEN NULL ELSE approved_ms END WHERE user_id=$1 AND id=$2")
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&claim.request_id).unwrap()).execute(&f.pool).await.unwrap();
        let saved = f
            .store
            .get_feed_value(&f.owner, &claim.request_id)
            .await
            .unwrap();
        assert_eq!(saved.status, "unknown");
        assert!(saved.snapshot.is_none());
        assert!(
            !f.store
                .begin_subscription_value(&claim, &proof)
                .await
                .unwrap()
        );
        assert_eq!(
            f.store
                .finish_subscription_value(&claim, Some(OUTPUT.to_vec()))
                .await
                .unwrap()
                .status,
            "unknown"
        );
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn value_dispatch_source_changes_clear_both_pending_and_completed_content() {
    let (f, sub) = setup().await;
    let proof = proof(&f).await;
    let complete = claim(&f).await;
    assert!(
        f.store
            .begin_subscription_value(&complete, &proof)
            .await
            .unwrap()
    );
    f.store
        .finish_subscription_value(&complete, Some(OUTPUT.to_vec()))
        .await
        .unwrap();
    let pending = claim(&f).await;
    assert!(
        f.store
            .begin_subscription_value(&pending, &proof)
            .await
            .unwrap()
    );
    f.store
        .delete_subscription(&f.owner, &sub.snapshot.subscription_id, 1)
        .await
        .unwrap();
    for c in [&pending, &complete] {
        let saved = f
            .store
            .finish_subscription_value(c, Some(OUTPUT.to_vec()))
            .await
            .unwrap();
        assert_eq!(saved.status, "invalidated");
        assert!(saved.snapshot.is_none() && saved.scores.is_none());
    }
    f.cleanup().await;
}

struct Runtime {
    proof: VerifiedSubscriptionConnection,
    calls: AtomicUsize,
    fail: bool,
}
impl SubscriptionValueRuntime for Runtime {
    fn verify<'a>(
        &'a self,
        _: &'a ValuePricing,
    ) -> BoxFuture<'a, Result<VerifiedSubscriptionConnection, ValueRuntimeError>> {
        Box::pin(async move {
            if self.fail {
                Err(ValueRuntimeError)
            } else {
                Ok(self.proof.clone())
            }
        })
    }
    fn score<'a>(
        &'a self,
        _: &'a ValuePricing,
        request: &'a personal_ai_llm::ChatRequest,
    ) -> BoxFuture<'a, Result<Vec<u8>, ValueRuntimeError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.messages.len(), 2);
            assert!(!request.messages[1].content.contains("source_url"));
            Ok(OUTPUT.to_vec())
        })
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn value_executor_uses_single_send_and_verification_failure_never_calls_model() {
    let (f, _) = setup().await;
    let mut runtime = Runtime {
        proof: proof(&f).await,
        calls: AtomicUsize::new(0),
        fail: false,
    };
    let saved = authorized(&f, true).await;
    let (a, b) = tokio::join!(
        execute_subscription_value(&f.store, &runtime, &f.owner, &saved.request_id),
        execute_subscription_value(&f.store, &runtime, &f.owner, &saved.request_id)
    );
    let results = [a.unwrap(), b.unwrap()];
    assert_eq!(results.iter().filter(|r| r.is_some()).count(), 1);
    assert_eq!(
        results.into_iter().flatten().next().unwrap().status,
        "succeeded"
    );
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
    runtime.fail = true;
    let saved = authorized(&f, true).await;
    assert_eq!(
        execute_subscription_value(&f.store, &runtime, &f.owner, &saved.request_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "unknown"
    );
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
    f.cleanup().await;
}

async fn deny_event(f: &Fixture, name: &str, event: &str) {
    sqlx::query(&format!("ALTER TABLE feed_value_audit ADD CONSTRAINT {name} CHECK(user_id<>'{}' OR event<>'{event}') NOT VALID", f.owner.as_str())).execute(&f.pool).await.unwrap();
}
async fn allow_events(f: &Fixture, name: &str) {
    sqlx::query(&format!(
        "ALTER TABLE feed_value_audit DROP CONSTRAINT {name}"
    ))
    .execute(&f.pool)
    .await
    .unwrap();
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn value_dispatch_audit_failures_roll_back_claim_send_and_completion_atomically() {
    let (f, _) = setup().await;
    let saved = authorized(&f, true).await;
    let proof = proof(&f).await;
    let constraint = format!("dispatch_fail_{}", Uuid::new_v4().simple());
    deny_event(&f, &constraint, "running").await;
    assert!(
        f.store
            .claim_subscription_value(&f.owner, &saved.request_id)
            .await
            .is_err()
    );
    allow_events(&f, &constraint).await;
    assert_eq!(
        f.store
            .get_feed_value(&f.owner, &saved.request_id)
            .await
            .unwrap()
            .status,
        "authorized"
    );
    let claim = f
        .store
        .claim_subscription_value(&f.owner, &saved.request_id)
        .await
        .unwrap()
        .unwrap();
    deny_event(&f, &constraint, "sending").await;
    assert!(
        f.store
            .begin_subscription_value(&claim, &proof)
            .await
            .is_err()
    );
    allow_events(&f, &constraint).await;
    assert!(
        f.store
            .begin_subscription_value(&claim, &proof)
            .await
            .unwrap()
    );
    deny_event(&f, &constraint, "succeeded").await;
    assert!(
        f.store
            .finish_subscription_value(&claim, Some(OUTPUT.to_vec()))
            .await
            .is_err()
    );
    allow_events(&f, &constraint).await;
    assert_eq!(
        f.store
            .get_feed_value(&f.owner, &claim.request_id)
            .await
            .unwrap()
            .status,
        "running"
    );
    assert!(
        !f.store
            .begin_subscription_value(&claim, &proof)
            .await
            .unwrap()
    );
    assert_eq!(
        f.store
            .finish_subscription_value(&claim, Some(OUTPUT.to_vec()))
            .await
            .unwrap()
            .status,
        "succeeded"
    );
    f.cleanup().await;
}

struct GatedRuntime {
    inner: Runtime,
    during_verify: bool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl SubscriptionValueRuntime for GatedRuntime {
    fn verify<'a>(
        &'a self,
        pricing: &'a ValuePricing,
    ) -> BoxFuture<'a, Result<VerifiedSubscriptionConnection, ValueRuntimeError>> {
        Box::pin(async move {
            if self.during_verify {
                self.entered.notify_one();
                self.release.notified().await;
            }
            self.inner.verify(pricing).await
        })
    }
    fn score<'a>(
        &'a self,
        pricing: &'a ValuePricing,
        request: &'a personal_ai_llm::ChatRequest,
    ) -> BoxFuture<'a, Result<Vec<u8>, ValueRuntimeError>> {
        Box::pin(async move {
            let output = self.inner.score(pricing, request).await;
            if !self.during_verify {
                self.entered.notify_one();
                self.release.notified().await;
            }
            output
        })
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn value_executor_rechecks_after_verification_and_discards_cancelled_inflight_output() {
    let (f, _) = setup().await;
    for during_verify in [true, false] {
        let runtime = GatedRuntime {
            inner: Runtime {
                proof: proof(&f).await,
                calls: AtomicUsize::new(0),
                fail: false,
            },
            during_verify,
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        };
        let saved = authorized(&f, true).await;
        let (result, ()) = tokio::join!(
            execute_subscription_value(&f.store, &runtime, &f.owner, &saved.request_id),
            async {
                runtime.entered.notified().await;
                f.store
                    .cancel_feed_value(&f.owner, &saved.request_id)
                    .await
                    .unwrap();
                runtime.release.notify_one();
            }
        );
        let result = result.unwrap().unwrap();
        assert_eq!(result.status, "cancelled");
        assert!(result.snapshot.is_none() && result.scores.is_none());
        assert_eq!(
            runtime.inner.calls.load(Ordering::SeqCst),
            usize::from(!during_verify)
        );
        assert!(
            execute_subscription_value(&f.store, &runtime, &f.owner, &saved.request_id)
                .await
                .unwrap()
                .is_none()
        );
    }
    f.cleanup().await;
}
