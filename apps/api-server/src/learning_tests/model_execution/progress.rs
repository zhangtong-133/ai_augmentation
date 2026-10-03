use super::*;
use personal_ai_agent_core::learning_model_execution::{
    observe_model_review, progress::ReviewProgressKind,
};
use personal_ai_llm::stream::TextDeltaSink;

struct Observed {
    runtime: Runtime,
    mode: &'static str,
}
impl SubscriptionReviewRuntime for Observed {
    fn verify<'a>(
        &'a self,
        item: &'a ModelAuthorization,
    ) -> BoxFuture<'a, Result<VerifiedSubscriptionConnection, ReviewRuntimeError>> {
        self.runtime.verify(item)
    }
    fn review<'a>(
        &'a self,
        item: &'a ModelAuthorization,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<Vec<u8>, ReviewRuntimeError>> {
        self.runtime.review(item, request)
    }
    fn review_observed<'a>(
        &'a self,
        item: &'a ModelAuthorization,
        request: &'a ChatRequest,
        sink: &'a dyn TextDeltaSink,
    ) -> BoxFuture<'a, Result<Vec<u8>, ReviewRuntimeError>> {
        Box::pin(async move {
            if self.mode == "pause" {
                self.runtime.calls.fetch_add(1, Ordering::SeqCst);
                sink.delta("private provisional text");
                return std::future::pending().await;
            }
            let bytes = self.runtime.review(item, request).await?;
            if self.mode == "overflow" {
                for _ in 0..32 {
                    sink.delta("x");
                }
            } else {
                sink.delta(std::str::from_utf8(&bytes).unwrap());
            }
            if self.mode == "fail" {
                Err(ReviewRuntimeError)
            } else {
                Ok(bytes)
            }
        })
    }
}
fn runtime(proof: VerifiedSubscriptionConnection, mode: &'static str) -> Observed {
    Observed {
        runtime: Runtime {
            proof,
            calls: AtomicUsize::new(0),
            fail: false,
        },
        mode,
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn observed_review_binds_events_and_reports_only_persisted_terminal_results() {
    for mode in ["success", "fail"] {
        let (f, _, request, proof) = authorized().await;
        let runtime = runtime(proof, mode);
        let (mut rx, execution) =
            observe_model_review(f.store.as_ref(), &runtime, &f.owner, &request);
        let saved = execution.await.unwrap().unwrap();
        assert_eq!(
            saved.status,
            if mode == "success" {
                "succeeded"
            } else {
                "unknown"
            }
        );
        let mut sequence = 0;
        let mut kinds = Vec::new();
        while let Some(event) = rx.recv().await {
            assert_eq!(event.owner, f.owner);
            assert_eq!(event.request_id, request);
            assert_eq!(event.sequence, sequence);
            sequence += 1;
            kinds.push(event.kind);
        }
        assert!(
            matches!(kinds.as_slice(), [ReviewProgressKind::Verifying, ReviewProgressKind::Sending, ReviewProgressKind::Delta(_), ReviewProgressKind::Persisting, ReviewProgressKind::Finished(status)] if status == &saved.status)
        );
        let (mut rx, replay) = observe_model_review(f.store.as_ref(), &runtime, &f.owner, &request);
        assert!(replay.await.unwrap().is_none());
        assert!(matches!(
            rx.recv().await.unwrap().kind,
            ReviewProgressKind::NotClaimed
        ));
        assert!(rx.recv().await.is_none());
        assert_eq!(runtime.runtime.calls.load(Ordering::SeqCst), 1);
        f.cleanup().await;
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn losing_progress_does_not_cancel_persistence_or_create_another_send() {
    for mode in ["overflow", "disconnect"] {
        let (f, _, request, proof) = authorized().await;
        let runtime = runtime(proof, mode);
        let (mut rx, execution) =
            observe_model_review(f.store.as_ref(), &runtime, &f.owner, &request);
        if mode == "disconnect" {
            rx.close();
        }
        let saved = execution.await.unwrap().unwrap();
        assert_eq!(saved.status, "succeeded");
        while let Some(event) = rx.recv().await {
            assert!(!matches!(event.kind, ReviewProgressKind::Finished(_)));
        }
        assert!(
            execute_model_review(f.store.as_ref(), &runtime, &f.owner, &request)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(runtime.runtime.calls.load(Ordering::SeqCst), 1);
        f.cleanup().await;
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn dropping_execution_closes_progress_and_does_not_restore_authorization() {
    let (f, _, request, proof) = authorized().await;
    let runtime = runtime(proof, "pause");
    let (mut rx, execution) = observe_model_review(f.store.as_ref(), &runtime, &f.owner, &request);
    let mut execution = Box::pin(execution);
    loop {
        tokio::select! {
            event = rx.recv() => { if matches!(event.unwrap().kind, ReviewProgressKind::Delta(_)) { break; } },
            _ = &mut execution => panic!("fixture must wait for cancellation"),
        }
    }
    drop(execution);
    assert!(rx.recv().await.is_none());
    assert!(
        f.store
            .claim_model_review(&f.owner, &request)
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query("UPDATE learning_model_authorizations SET dispatch_deadline_ms=0 WHERE user_id=$1")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .get_model_authorization(&f.owner, &request)
            .await
            .unwrap()
            .status,
        "unknown"
    );
    assert_eq!(runtime.runtime.calls.load(Ordering::SeqCst), 1);
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn failed_persistence_never_notifies_success_or_reclaims_the_sent_request() {
    let (f, _, request, proof) = authorized().await;
    let runtime = runtime(proof, "success");
    let owner = Uuid::parse_str(f.owner.as_str()).unwrap();
    let constraint = format!("progress_failure_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("ALTER TABLE learning_model_authorization_audit ADD CONSTRAINT {constraint} CHECK(user_id<>'{owner}'::uuid OR event<>'succeeded')")).execute(&f.pool).await.unwrap();
    let (mut rx, execution) = observe_model_review(f.store.as_ref(), &runtime, &f.owner, &request);
    let result = execution.await;
    sqlx::query(&format!(
        "ALTER TABLE learning_model_authorization_audit DROP CONSTRAINT {constraint}"
    ))
    .execute(&f.pool)
    .await
    .unwrap();
    assert!(result.is_err());
    let mut failed = false;
    while let Some(event) = rx.recv().await {
        assert!(!matches!(event.kind, ReviewProgressKind::Finished(_)));
        failed |= matches!(event.kind, ReviewProgressKind::StorageFailure);
    }
    assert!(failed);
    assert!(
        f.store
            .claim_model_review(&f.owner, &request)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(runtime.runtime.calls.load(Ordering::SeqCst), 1);
    f.cleanup().await;
}
