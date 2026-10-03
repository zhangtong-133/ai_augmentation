//! One explicitly authorized learning review; no background polling or inference retries.
use personal_ai_domain::UserId;
use personal_ai_llm::{ChatMessage, ChatRequest, Role};
use personal_ai_storage::{
    BoxFuture, StorageResult,
    learning::model_authorization::{ModelAuthorization, ModelReviewExecutionStore},
    subscription_connections::VerifiedSubscriptionConnection,
};
use std::time::Duration;
#[derive(Debug)]
pub struct ReviewRuntimeError;
/// Implementations pin one local account across verification and the single send.
pub trait SubscriptionReviewRuntime: Send + Sync {
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
    let Some(claim) = store.claim_model_review(owner, request).await? else {
        return Ok(None);
    };
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
            tokio::time::timeout(
                Duration::from_secs(60),
                runtime.review(&claim.authorization, &request),
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
    store.finish_model_review(&claim, output).await.map(Some)
}
