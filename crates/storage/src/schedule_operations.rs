//! 管理员只读核对：结构与队列元数据，不读取私有正文或租约凭据。
use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;

#[derive(Debug, serde::Serialize)]
pub struct ScheduleAuditCounts {
    pub tasks: i64,
    pub drafts: i64,
    pub expired_drafts: i64,
    pub scheduled: i64,
    /// scheduled 中已到期的子集。
    pub due: i64,
    pub running: i64,
    /// running 中可重新领取的子集，不视为结构不一致。
    pub expired_leases: i64,
    pub delivered: i64,
    pub cancelled: i64,
    pub failed: i64,
    pub reminders: i64,
    pub inconsistent: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct ScheduleAuditItem {
    pub request_id: String,
    pub version: String,
    pub status: String,
    pub run_at_unix_ms: String,
    pub created_at_unix_ms: String,
    pub approval_expires_at_unix_ms: String,
    pub approved_at_unix_ms: Option<String>,
    pub lease_until_unix_ms: Option<String>,
    pub delivered_at_unix_ms: Option<String>,
    pub cancelled_at_unix_ms: Option<String>,
    pub reminder_delivered_at_unix_ms: Option<String>,
    pub reminder_present: bool,
    pub ready_to_claim: bool,
    pub issues: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ScheduleAudit {
    pub user_id: String,
    pub snapshot_at_unix_ms: String,
    /// 没有进程心跳，不能从积压推断进程在线或配置开关。
    pub worker_liveness: &'static str,
    pub counts: ScheduleAuditCounts,
    pub ready_to_claim: i64,
    pub oldest_ready_run_at_unix_ms: Option<String>,
    pub oldest_ready_delay_ms: Option<String>,
    pub consistent: bool,
    pub items: Vec<ScheduleAuditItem>,
    pub next_cursor: Option<String>,
}

pub trait ScheduleOperationsStore: Send + Sync {
    /// 同一个只读快照核对全量汇总与当前页，每页至多 100 条。
    fn audit_schedules(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ScheduleAudit>>;
}
