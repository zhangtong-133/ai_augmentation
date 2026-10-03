//! 内部评分报价及同意仓储；本阶段不预留金额、不提供发送凭据。
use crate::{BoxFuture, StorageResult, feeds::FeedPage, model_agents::ModelCallBudget};
use personal_ai_domain::UserId;
use personal_ai_feeds::brief::BriefCandidate;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueSnapshot {
    pub day_start_unix_ms: u64,
    pub as_of_unix_ms: u64,
    pub preference_revision: u64,
    pub keywords: Vec<String>,
    pub candidates: Vec<BriefCandidate>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValuePricing {
    Api {
        budget: ModelCallBudget,
        request_limit: i64,
    },
    /// 订阅额度不是零美元报价；`connection_id` 必须由服务端绑定当前用户。
    Subscription {
        provider: String,
        model: String,
        configuration_version: String,
        connection_id: String,
        valid_until_unix_ms: i64,
    },
}
/// API 模式须重建请求并离线计数；订阅模式须绑定已验证连接。禁止网络 I/O 或客户端报价。
pub trait ValueQuotePlanner: Send + Sync {
    /// # Errors
    /// API 计费上界无法证明或订阅连接未通过服务端验证时必须拒绝。
    fn quote(
        &self,
        owner: &UserId,
        request: &str,
        snapshot: &ValueSnapshot,
    ) -> StorageResult<ValuePricing>;
}
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ValueReview {
    pub request_id: String,
    pub status: String,
    pub digest: String,
    pub amount: Option<i64>,
    pub pricing: ValuePricing,
    pub created_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub approved_at_unix_ms: Option<i64>,
    /// 取消、到期或失效后清除内容；元数据与审计保留。
    pub snapshot: Option<ValueSnapshot>,
}
#[derive(Clone)]
pub struct ValueApproval {
    pub digest: String,
    pub currency: Option<String>,
    pub amount: Option<i64>,
    pub acknowledge_sharing: bool,
    pub acknowledge_cost: bool,
    pub acknowledge_subscription_usage: bool,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ValueAudit {
    pub event: String,
    pub at_unix_ms: i64,
}
pub trait FeedValueStore: Send + Sync {
    fn preview_feed_value(
        &self,
        owner: &UserId,
        request: &str,
        planner: Arc<dyn ValueQuotePlanner>,
    ) -> BoxFuture<'_, StorageResult<ValueReview>>;
    fn approve_feed_value(
        &self,
        owner: &UserId,
        request: &str,
        approval: &ValueApproval,
    ) -> BoxFuture<'_, StorageResult<ValueReview>>;
    fn cancel_feed_value(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ValueReview>>;
    fn get_feed_value(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ValueReview>>;
    fn list_feed_values(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedPage<ValueReview>>>;
    fn feed_value_audit(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Vec<ValueAudit>>>;
}
