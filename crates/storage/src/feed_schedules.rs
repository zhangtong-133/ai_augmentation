//! 周期 RSS 采集授权仓储；保存同意不代表领取或执行。
use crate::{BoxFuture, StorageResult, feeds::FeedPage};
use personal_ai_domain::UserId;
use personal_ai_feeds::schedule::{ScheduleInput, SchedulePlan};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedScheduleStatus {
    Draft,
    Active,
    Cancelled,
    Expired,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct FeedSchedule {
    pub plan: SchedulePlan,
    pub digest: String,
    pub status: FeedScheduleStatus,
    pub approved_at_unix_ms: Option<i64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeedScheduleAudit {
    pub event: String,
    pub at_unix_ms: i64,
}
pub trait FeedScheduleStore: Send + Sync {
    fn preview_feed_schedule(
        &self,
        owner: &UserId,
        subscription: &str,
        input: &ScheduleInput,
    ) -> BoxFuture<'_, StorageResult<FeedSchedule>>;
    fn approve_feed_schedule(
        &self,
        owner: &UserId,
        schedule: &str,
        digest: &str,
    ) -> BoxFuture<'_, StorageResult<FeedSchedule>>;
    fn cancel_feed_schedule(
        &self,
        owner: &UserId,
        schedule: &str,
    ) -> BoxFuture<'_, StorageResult<FeedSchedule>>;
    fn get_feed_schedule(
        &self,
        owner: &UserId,
        schedule: &str,
    ) -> BoxFuture<'_, StorageResult<FeedSchedule>>;
    fn list_feed_schedules(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedPage<FeedSchedule>>>;
    fn feed_schedule_audit(
        &self,
        owner: &UserId,
        schedule: &str,
    ) -> BoxFuture<'_, StorageResult<Vec<FeedScheduleAudit>>>;
}

/// 内部周期采集领取与发送栅栏；不从 HTTP 暴露，也不接受客户端计划。
pub trait FeedScheduleExecutionStore: Send + Sync {
    fn claim_scheduled_collection(
        &self,
        owner: &UserId,
        schedule: &str,
    ) -> BoxFuture<'_, StorageResult<crate::feeds::CollectionClaim>>;
    /// 每份凭据只成功一次；再次检查当前授权、订阅及派发期限。
    fn dispatch_scheduled_collection(
        &self,
        claim: &crate::feeds::CollectionClaim,
    ) -> BoxFuture<'_, StorageResult<bool>>;
}
