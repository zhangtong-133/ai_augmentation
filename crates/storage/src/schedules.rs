//! 一次性站内提醒的不可变预览、精确授权和取消协议。
use crate::{BoxFuture, StorageError, StorageResult};
use personal_ai_domain::UserId;

pub const SCHEDULE_VERSION: &str = "local-reminder-once-v1";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewSchedule {
    pub request_id: String,
    pub title: String,
    pub body: String,
    pub run_at_unix_ms: i64,
}
impl NewSchedule {
    /// # Errors
    /// 标题限制 80 字符，正文限制 2000 字符；拒绝空白、NUL 和非法时间。
    pub fn validate(&self) -> StorageResult<()> {
        if self.title.trim().is_empty()
            || self.title.chars().count() > 80
            || self.body.trim().is_empty()
            || self.body.chars().count() > 2000
            || self.title.contains('\0')
            || self.body.contains('\0')
            || self.run_at_unix_ms <= 0
        {
            return Err(StorageError::InvalidData("invalid schedule".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleApproval {
    pub digest: String,
    pub accepted_run_at_unix_ms: i64,
    pub accepted_max_runs: u32,
    pub accepted_amount_micro: i64,
    pub acknowledge_schedule: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Schedule {
    pub request_id: String,
    pub version: String,
    pub title: String,
    pub body: String,
    pub run_at_unix_ms: i64,
    pub max_runs: u32,
    pub amount_micro: i64,
    pub digest: String,
    pub status: String,
    pub created_at_unix_ms: i64,
    pub approval_expires_at_unix_ms: i64,
    pub approved_at_unix_ms: Option<i64>,
    pub cancelled_at_unix_ms: Option<i64>,
    pub delivered_at_unix_ms: Option<i64>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct SchedulePage {
    pub items: Vec<Schedule>,
    pub next_cursor: Option<String>,
}

pub trait ScheduleStore: Send + Sync {
    fn create_schedule(
        &self,
        owner: &UserId,
        input: &NewSchedule,
    ) -> BoxFuture<'_, StorageResult<Schedule>>;
    fn get_schedule(&self, owner: &UserId, request: &str)
    -> BoxFuture<'_, StorageResult<Schedule>>;
    fn list_schedules(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<SchedulePage>>;
    fn approve_schedule(
        &self,
        owner: &UserId,
        request: &str,
        approval: &ScheduleApproval,
    ) -> BoxFuture<'_, StorageResult<Schedule>>;
    fn cancel_schedule(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Schedule>>;
}

/// 内部租约凭据，不从 HTTP 接受，不包含可替换的提醒正文。
#[derive(Clone, Debug)]
pub struct ScheduleLease {
    pub owner: UserId,
    pub request_id: String,
    pub claim_id: String,
    pub lease_until_unix_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ScheduleReminder {
    pub request_id: String,
    pub title: String,
    pub body: String,
    pub delivered_at_unix_ms: i64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ReminderPage {
    pub items: Vec<ScheduleReminder>,
    pub next_cursor: Option<String>,
}

pub trait ScheduleDeliveryStore: Send + Sync {
    /// 内部跨用户领取；事务复核授权后返回，过期租约可被新凭据替换。
    fn claim_due_schedule(&self) -> BoxFuture<'_, StorageResult<Option<ScheduleLease>>>;
    /// 只使用保存的正文；旧租约、取消或授权改变时不投递。
    fn deliver_schedule(
        &self,
        lease: &ScheduleLease,
    ) -> BoxFuture<'_, StorageResult<ScheduleReminder>>;
    fn list_schedule_reminders(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ReminderPage>>;
}
