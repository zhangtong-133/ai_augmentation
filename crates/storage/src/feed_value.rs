//! 评分报价、同意和订阅执行仓储；API 金额预留仍独立于订阅路径。
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
    /// 精确本机计算同意，不包含订阅身份或 API 金额。
    Local {
        endpoint: String,
        model: String,
        profile: String,
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
    pub scores: Option<Vec<ValueScore>>,
}
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools)] // 保留各项独立同意，仓储拒绝遗漏或混合授权。
pub struct ValueApproval {
    pub digest: String,
    pub currency: Option<String>,
    pub amount: Option<i64>,
    pub acknowledge_sharing: bool,
    pub acknowledge_cost: bool,
    pub acknowledge_subscription_usage: bool,
    pub acknowledge_local_compute: bool,
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

/// 完整校验后的模型建议；id 是冻结候选中的临时编号。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueScore {
    pub id: usize,
    #[serde(deserialize_with = "explicit_score")]
    pub score: Option<u8>,
    pub reason: String,
}
fn explicit_score<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u8>, D::Error> {
    Option::<u8>::deserialize(d)
}
/// 内部一次领取凭据，不可反序列化或输出；重启后不能重领。
pub struct ValueClaim {
    pub owner: UserId,
    pub request_id: String,
    pub token: String,
    pub review: ValueReview,
}
pub trait FeedValueExecutionStore: Send + Sync {
    fn claim_subscription_value(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Option<ValueClaim>>>;
    /// 发送线性化点：匹配最新本地凭据身份与连接版本，并持久化单次发送标记。
    fn begin_subscription_value(
        &self,
        claim: &ValueClaim,
        proof: &crate::subscription_connections::VerifiedSubscriptionConnection,
    ) -> BoxFuture<'_, StorageResult<bool>>;
    /// None 表示未知/失败；有输出也须重新校验完整协议，不保存原始错误正文。
    fn finish_subscription_value(
        &self,
        claim: &ValueClaim,
        output: Option<Vec<u8>>,
    ) -> BoxFuture<'_, StorageResult<ValueReview>>;
}

pub trait LocalValueExecutionStore: Send + Sync {
    fn claim_local_value(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Option<ValueClaim>>>;
    fn begin_local_value(
        &self,
        claim: &ValueClaim,
        target: &personal_ai_llm::local::LocalTarget,
    ) -> BoxFuture<'_, StorageResult<bool>>;
    fn finish_local_value(
        &self,
        claim: &ValueClaim,
        output: Option<Vec<u8>>,
    ) -> BoxFuture<'_, StorageResult<ValueReview>>;
}
