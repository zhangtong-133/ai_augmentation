//! Internal subscription-only executor. No worker, HTTP entry point or API-money fallback.
use crate::feed_value::plan_value_scoring;
use personal_ai_domain::UserId;
use personal_ai_llm::ChatRequest;
use personal_ai_storage::{
    BoxFuture, StorageResult,
    feed_value::{FeedValueExecutionStore, ValuePricing, ValueReview},
    subscription_connections::VerifiedSubscriptionConnection,
};
use std::time::Duration;

/// Do not carry provider bodies, credentials or account identifiers in diagnostics.
#[derive(Debug)]
pub struct ValueRuntimeError;

/// Trusted local runtime port. Implementations must pin the same account/credentials across
/// verification and send, verify current plan scopes and model access, and never retry inference.
/// Only messages are portable to a subscription transport: API temperature/token-budget fields
/// must not be treated as supported provider parameters or guaranteed subscription usage caps.
pub trait SubscriptionValueRuntime: Send + Sync {
    fn verify<'a>(
        &'a self,
        pricing: &'a ValuePricing,
    ) -> BoxFuture<'a, Result<VerifiedSubscriptionConnection, ValueRuntimeError>>;
    fn score<'a>(
        &'a self,
        pricing: &'a ValuePricing,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<Vec<u8>, ValueRuntimeError>>;
}

/// Execute at most one attempt for one explicitly authorized review. API reviews cannot be claimed.
/// Unknown outcomes, dropped futures and failed persistence never restore permission to send.
/// # Errors
/// Storage failures leave the durable claim in place; subsequent reads recover its deadline.
pub async fn execute_subscription_value(
    store: &dyn FeedValueExecutionStore,
    runtime: &dyn SubscriptionValueRuntime,
    owner: &UserId,
    request_id: &str,
) -> StorageResult<Option<ValueReview>> {
    let Some(claim) = store.claim_subscription_value(owner, request_id).await? else {
        return Ok(None);
    };
    let proof = tokio::time::timeout(
        Duration::from_secs(10),
        runtime.verify(&claim.review.pricing),
    )
    .await;
    let output = if let Ok(Ok(proof)) = proof {
        let plan = claim.review.snapshot.as_ref().and_then(|s| {
            plan_value_scoring(
                &claim.owner,
                &claim.request_id,
                s.day_start_unix_ms,
                s.as_of_unix_ms,
                &s.keywords,
                &s.candidates,
            )
            .ok()
            .flatten()
        });
        if let Some(plan) = plan {
            if store.begin_subscription_value(&claim, &proof).await? {
                tokio::time::timeout(
                    Duration::from_mins(1),
                    runtime.score(&claim.review.pricing, &plan.request()),
                )
                .await
                .ok()
                .and_then(Result::ok)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    store
        .finish_subscription_value(&claim, output)
        .await
        .map(Some)
}
