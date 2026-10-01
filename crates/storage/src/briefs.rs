//! 当前用户的日报偏好和不可变计划；所有操作均无网络或模型执行。
use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;
use personal_ai_feeds::brief::BriefPlan;
use serde::Serialize;

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct BriefPreferences {
    /// 尚未设置偏好时为 0，首次保存为 1。
    pub revision: u64,
    pub keywords: Vec<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BriefStatus {
    Ready,
    Invalidated,
    Deleted,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct SavedBrief {
    pub request_id: String,
    pub preference_revision: u64,
    pub day_start_unix_ms: u64,
    pub created_at_unix_ms: u64,
    pub status: BriefStatus,
    pub digest: String,
    pub plan: Option<BriefPlan>,
}
/// 历史列表不加载或返回正文和偏好，详情由显式 get 查询。
#[derive(Clone, Serialize)]
pub struct BriefSummary {
    pub request_id: String,
    pub day_start_unix_ms: u64,
    pub created_at_unix_ms: u64,
    pub status: BriefStatus,
}
#[derive(Clone, Serialize)]
pub struct BriefPage {
    pub items: Vec<BriefSummary>,
    pub next_cursor: Option<String>,
}
pub trait BriefStore: Send + Sync {
    fn get_brief_preferences(
        &self,
        owner: &UserId,
    ) -> BoxFuture<'_, StorageResult<BriefPreferences>>;
    fn save_brief_preferences(
        &self,
        owner: &UserId,
        expected_revision: u64,
        keywords: &[String],
    ) -> BoxFuture<'_, StorageResult<BriefPreferences>>;
    /// 仅创建数据库时钟的当天计划；重复请求返回原记录，不重算。
    fn create_brief(
        &self,
        owner: &UserId,
        request: &str,
        expected_preference_revision: u64,
    ) -> BoxFuture<'_, StorageResult<SavedBrief>>;
    fn get_brief(&self, owner: &UserId, request: &str) -> BoxFuture<'_, StorageResult<SavedBrief>>;
    fn list_briefs(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<BriefPage>>;
    /// 保留请求墓碑，清除正文，重试 create 不会复活。
    fn delete_brief(&self, owner: &UserId, request: &str) -> BoxFuture<'_, StorageResult<()>>;
}
