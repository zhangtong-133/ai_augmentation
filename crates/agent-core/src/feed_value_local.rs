//! 固定本地评分协议及一次性执行，与订阅凭据隔离。
use crate::feed_value::{ValueError, ValueScoringPlan, plan_value_scoring};
use personal_ai_domain::UserId;
use personal_ai_llm::{
    ChatRequest,
    local::{LocalInference, LocalTarget, MAX_PROMPT_BYTES, OUTPUT_TOKENS},
    stream::IgnoreTextDeltas,
};
use personal_ai_storage::{
    StorageResult,
    feed_value::{LocalValueExecutionStore, ValuePricing, ValueReview, ValueSnapshot},
};
use std::time::Duration;

pub const LOCAL_VALUE_PROFILE: &str = "local-rss-v1";

/// 本地输出限制是授权配置的一部分，超长输入拒绝而非截断。
/// # Errors
/// 材料超出项目本地上下文预算时失败。
pub fn local_request(plan: &ValueScoringPlan) -> Result<ChatRequest, ValueError> {
    let mut request = plan.request();
    if request
        .messages
        .iter()
        .map(|m| m.content.len())
        .sum::<usize>()
        > MAX_PROMPT_BYTES
    {
        return Err(ValueError::TooLarge);
    }
    request.max_output_tokens = Some(OUTPUT_TOKENS);
    Ok(request)
}
/// # Errors
/// 快照不能重建或超出本地限制时失败。
pub fn validate_local_snapshot(
    owner: &UserId,
    request: &str,
    snapshot: &ValueSnapshot,
) -> Result<(), ValueError> {
    let plan = plan_value_scoring(
        owner,
        request,
        snapshot.day_start_unix_ms,
        snapshot.as_of_unix_ms,
        &snapshot.keywords,
        &snapshot.candidates,
    )?
    .ok_or(ValueError::InvalidSnapshot)?;
    local_request(&plan).map(|_| ())
}
/// 单个已授权请求最多派发一次；未知、取消或持久化失败均不重发。
/// # Errors
/// 仓储失败保留已领取状态。
pub async fn execute_local_value(
    store: &dyn LocalValueExecutionStore,
    runtime: &dyn LocalInference,
    target: &LocalTarget,
    owner: &UserId,
    request: &str,
) -> StorageResult<Option<ValueReview>> {
    let Some(claim) = store.claim_local_value(owner, request).await? else {
        return Ok(None);
    };
    let bound = matches!(&claim.review.pricing, ValuePricing::Local { endpoint, model, profile, .. } if endpoint == target.endpoint() && model == target.model() && profile == LOCAL_VALUE_PROFILE);
    let chat = claim
        .review
        .snapshot
        .as_ref()
        .and_then(|s| {
            plan_value_scoring(
                owner,
                request,
                s.day_start_unix_ms,
                s.as_of_unix_ms,
                &s.keywords,
                &s.candidates,
            )
            .ok()
            .flatten()
        })
        .and_then(|plan| local_request(&plan).ok());
    let output = if let Some(chat) = chat.filter(|_| bound) {
        if store.begin_local_value(&claim, target).await? {
            tokio::time::timeout(
                Duration::from_secs(60),
                runtime.infer(target, &chat, &IgnoreTextDeltas),
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
    store.finish_local_value(&claim, output).await.map(Some)
}
