//! One explicitly authorized learning review; no background polling or inference retries.
use personal_ai_domain::UserId;
use personal_ai_llm::stream::TextDeltaSink;
use personal_ai_llm::{ChatMessage, ChatRequest, Role};
use personal_ai_storage::{
    BoxFuture, StorageResult,
    learning::model_authorization::{ModelAuthorization, ModelReviewExecutionStore},
    subscription_connections::VerifiedSubscriptionConnection,
};
use std::time::Duration;
pub mod local;
pub mod progress;
pub mod text_bridge;
use progress::{Publisher, ReviewProgressEvent, ReviewProgressKind};
#[derive(Debug)]
pub struct ReviewRuntimeError;
/// Implementations pin one local account across verification and the single send.
pub trait SubscriptionReviewRuntime: Send + Sync {
    fn review_observed<'a>(
        &'a self,
        authorization: &'a ModelAuthorization,
        request: &'a ChatRequest,
        _sink: &'a dyn TextDeltaSink,
    ) -> BoxFuture<'a, Result<Vec<u8>, ReviewRuntimeError>> {
        self.review(authorization, request)
    }

    fn verify<'a>(
        &'a self,
        authorization: &'a ModelAuthorization,
    ) -> BoxFuture<'a, Result<VerifiedSubscriptionConnection, ReviewRuntimeError>>;
    fn review<'a>(
        &'a self,
        authorization: &'a ModelAuthorization,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<Vec<u8>, ReviewRuntimeError>>;
}
/// # Errors
/// A storage failure leaves the durable claim consumed. A later read recovers its deadline.
pub async fn execute_model_review(
    store: &dyn ModelReviewExecutionStore,
    runtime: &dyn SubscriptionReviewRuntime,
    owner: &UserId,
    request: &str,
) -> StorageResult<Option<ModelAuthorization>> {
    let (_, execution) = observe_model_review(store, runtime, owner, request);
    execution.await
}

/// Creates a private receiver and an unspawned execution future for one request.
/// Dropping the receiver stops notifications only; dropping the future stops local
/// execution, leaves the durable claim consumed, and closes the notification channel.
/// EOF without Finished/NotClaimed/StorageFailure is incomplete: discard previews.
pub fn observe_model_review<'a>(
    store: &'a dyn ModelReviewExecutionStore,
    runtime: &'a dyn SubscriptionReviewRuntime,
    owner: &'a UserId,
    request: &'a str,
) -> (
    tokio::sync::mpsc::Receiver<ReviewProgressEvent>,
    impl std::future::Future<Output = StorageResult<Option<ModelAuthorization>>> + Send + 'a,
) {
    let (publisher, receiver) = Publisher::channel(owner, request);
    let execution = async move {
        let result = execute_observed(store, runtime, owner, request, &publisher).await;
        if result.is_err() {
            publisher.emit(ReviewProgressKind::StorageFailure);
        }
        result
    };
    (receiver, execution)
}
async fn execute_observed(
    store: &dyn ModelReviewExecutionStore,
    runtime: &dyn SubscriptionReviewRuntime,
    owner: &UserId,
    request: &str,
    publisher: &Publisher,
) -> StorageResult<Option<ModelAuthorization>> {
    let Some(claim) = store.claim_model_review(owner, request).await? else {
        publisher.emit(ReviewProgressKind::NotClaimed);
        return Ok(None);
    };
    publisher.emit(ReviewProgressKind::Verifying);
    let proof = tokio::time::timeout(
        Duration::from_secs(10),
        runtime.verify(&claim.authorization),
    )
    .await;
    let request = claim.authorization.preview.as_ref().and_then(|p| {
        serde_json::to_string(p.input())
            .ok()
            .map(|input| ChatRequest {
                messages: vec![
                    ChatMessage {
                        role: Role::System,
                        content: p.system_prompt().into(),
                    },
                    ChatMessage {
                        role: Role::User,
                        content: input,
                    },
                ],
                temperature: None,
                max_output_tokens: None,
            })
    });
    let output = if let (Ok(Ok(proof)), Some(request)) = (proof, request) {
        if store.begin_model_review(&claim, &proof).await? {
            publisher.emit(ReviewProgressKind::Sending);
            tokio::time::timeout(
                Duration::from_mins(1),
                runtime.review_observed(&claim.authorization, &request, publisher),
            )
            .await
            .ok()
            .and_then(Result::ok)
        } else {
            None
        }
    } else {
        None
    };
    publisher.emit(ReviewProgressKind::Persisting);
    let saved = store.finish_model_review(&claim, output).await?;
    publisher.emit(ReviewProgressKind::Finished(saved.status.clone()));
    Ok(Some(saved))
}
