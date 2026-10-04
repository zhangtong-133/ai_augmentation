//! 订阅核验授权与内部一次性执行端口；不保存原始证据副本。
use personal_ai_learning::model_review::{ModelReviewPreview, ModelReviewResponse};
use serde::Serialize;
#[derive(Clone)]
pub struct ModelAuthorizationInput {
    pub request_id: String,
    pub connection_id: String,
    pub connection_revision: u64,
    pub model: String,
}
#[derive(Clone)]
pub struct ModelApproval {
    pub digest: String,
    pub acknowledge_sharing: bool,
    pub acknowledge_subscription_usage: bool,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ModelAuthorization {
    /// Some means independent local inference; subscription connection fields are empty/zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_endpoint: Option<String>,
    pub request_id: String,
    pub plan_id: String,
    pub task_id: String,
    pub connection_id: String,
    pub connection_revision: u64,
    pub model: String,
    pub status: String,
    pub digest: String,
    pub created_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub approved_at_unix_ms: Option<u64>,
    /// 仅从当前有效来源重建，取消/到期/失效时为空。
    pub preview: Option<ModelReviewPreview>,
    pub advice: Option<ModelReviewResponse>,
}

#[derive(Clone)]
pub struct LocalModelAuthorizationInput {
    pub request_id: String,
    pub target: personal_ai_llm::local::LocalTarget,
}
#[derive(Clone)]
pub struct LocalModelApproval {
    pub digest: String,
    pub acknowledge_sharing: bool,
    pub acknowledge_local_compute: bool,
}
/// Local dispatch is deliberately separate from subscription proof and claiming.
pub trait LocalReviewExecutionStore: ModelReviewExecutionStore {
    fn claim_local_review(
        &self,
        owner: &personal_ai_domain::UserId,
        request: &str,
    ) -> crate::BoxFuture<'_, crate::StorageResult<Option<ModelReviewClaim>>>;
    fn begin_local_review(
        &self,
        claim: &ModelReviewClaim,
        target: &personal_ai_llm::local::LocalTarget,
    ) -> crate::BoxFuture<'_, crate::StorageResult<bool>>;
}
#[derive(Serialize)]
pub struct ModelAuthorizationPage {
    pub items: Vec<ModelAuthorization>,
    pub next_cursor: Option<String>,
}

/// 仅内部使用，不序列化或从 HTTP 接收；令牌在崩溃或取消后不得重置。
pub struct ModelReviewClaim {
    pub owner: personal_ai_domain::UserId,
    pub request_id: String,
    pub token: String,
    pub authorization: ModelAuthorization,
}
pub trait ModelReviewExecutionStore: Send + Sync {
    fn claim_model_review(
        &self,
        owner: &personal_ai_domain::UserId,
        request: &str,
    ) -> crate::BoxFuture<'_, crate::StorageResult<Option<ModelReviewClaim>>>;
    fn begin_model_review(
        &self,
        claim: &ModelReviewClaim,
        proof: &crate::subscription_connections::VerifiedSubscriptionConnection,
    ) -> crate::BoxFuture<'_, crate::StorageResult<bool>>;
    fn finish_model_review(
        &self,
        claim: &ModelReviewClaim,
        output: Option<Vec<u8>>,
    ) -> crate::BoxFuture<'_, crate::StorageResult<ModelAuthorization>>;
}
