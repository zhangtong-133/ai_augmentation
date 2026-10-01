//! 显式授权的本地定时日报，不触发采集或模型。
use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BriefSchedule {
    pub revision: u64,
    pub enabled: bool,
    pub minute_utc: u16,
    pub next_run_unix_ms: Option<i64>,
    pub last_attempt_unix_ms: Option<i64>,
    pub last_request_id: Option<String>,
    pub last_outcome: Option<String>,
}
impl Default for BriefSchedule {
    fn default() -> Self {
        Self {
            revision: 0,
            enabled: false,
            minute_utc: 540,
            next_run_unix_ms: None,
            last_attempt_unix_ms: None,
            last_request_id: None,
            last_outcome: None,
        }
    }
}
pub trait BriefScheduleStore: Send + Sync {
    fn get_brief_schedule(&self, owner: &UserId) -> BoxFuture<'_, StorageResult<BriefSchedule>>;
    fn save_brief_schedule(
        &self,
        owner: &UserId,
        revision: u64,
        enabled: bool,
        minute_utc: u16,
    ) -> BoxFuture<'_, StorageResult<BriefSchedule>>;
    /// 调度进程专用：事务内处理一个到期用户；无任务时 false，已处理（包括跳过）时 true。
    fn generate_due_brief(&self) -> BoxFuture<'_, StorageResult<bool>>;
}
