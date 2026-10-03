//! 订阅核验的精确授权元数据；不派发模型、不保存证据副本。
use personal_ai_learning::model_review::ModelReviewPreview;
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
#[derive(Clone, Serialize)]
pub struct ModelAuthorization {
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
}
#[derive(Serialize)]
pub struct ModelAuthorizationPage {
    pub items: Vec<ModelAuthorization>,
    pub next_cursor: Option<String>,
}
