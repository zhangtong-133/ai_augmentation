//! Independently authorized local review, sharing source/notification lifecycle only.
use super::progress::{Publisher, ReviewProgressEvent, ReviewProgressKind};
use personal_ai_domain::UserId;
use personal_ai_llm::{
    ChatMessage, ChatRequest, Role,
    local::{LocalInference, LocalTarget},
};
use personal_ai_storage::{
    StorageResult,
    learning::model_authorization::{LocalReviewExecutionStore, ModelAuthorization},
};
use std::time::Duration;

/// No automatic target selection or retry. Dropping execution consumes any durable claim.
pub fn observe_local_review<'a>(
    store: &'a dyn LocalReviewExecutionStore,
    runtime: &'a dyn LocalInference,
    target: &'a LocalTarget,
    owner: &'a UserId,
    request: &'a str,
) -> (
    tokio::sync::mpsc::Receiver<ReviewProgressEvent>,
    impl std::future::Future<Output = StorageResult<Option<ModelAuthorization>>> + Send + 'a,
) {
    let (publisher, receiver) = Publisher::channel(owner, request);
    let execution = async move {
        let result = execute(store, runtime, target, owner, request, &publisher).await;
        if result.is_err() {
            publisher.emit(ReviewProgressKind::StorageFailure);
        }
        result
    };
    (receiver, execution)
}
async fn execute(
    store: &dyn LocalReviewExecutionStore,
    runtime: &dyn LocalInference,
    target: &LocalTarget,
    owner: &UserId,
    request: &str,
    publisher: &Publisher,
) -> StorageResult<Option<ModelAuthorization>> {
    // The explicit command checks target before claiming; the executor rechecks it before sending.
    let Some(claim) = store.claim_local_review(owner, request).await? else {
        publisher.emit(ReviewProgressKind::NotClaimed);
        return Ok(None);
    };
    publisher.emit(ReviewProgressKind::Verifying);
    let chat = claim.authorization.preview.as_ref().and_then(|preview| {
        serde_json::to_string(preview.input())
            .ok()
            .map(|input| ChatRequest {
                messages: vec![
                    ChatMessage {
                        role: Role::System,
                        content: preview.system_prompt().into(),
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
    let bound = claim.authorization.local_endpoint.as_deref() == Some(target.endpoint())
        && claim.authorization.model == target.model();
    let output = if let Some(chat) = chat.filter(|_| bound) {
        if store.begin_local_review(&claim, target).await? {
            publisher.emit(ReviewProgressKind::Sending);
            tokio::time::timeout(
                Duration::from_secs(60),
                runtime.infer(target, &chat, publisher),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .map(String::into_bytes)
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
