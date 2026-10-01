//! RSS 只读运维元数据，不读取来源、计划、正文或执行凭据。
use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;
use std::collections::BTreeMap;

#[derive(Debug, serde::Serialize)]
pub struct FeedAuditItem {
    pub request_id: String,
    pub subscription_id: String,
    pub status: String,
    pub reason: Option<String>,
    pub created_at_unix_ms: String,
    pub deadline_unix_ms: Option<String>,
    pub issues: Vec<String>,
}
#[derive(Debug, serde::Serialize)]
pub struct FeedAudit {
    pub user_id: String,
    pub snapshot_at_unix_ms: String,
    /// 全用户汇总，不随分页游标变化。
    pub counts: BTreeMap<String, i64>,
    pub remaining_previews: i64,
    pub remaining_collections_today: i64,
    pub consistent: bool,
    pub issues: Vec<String>,
    pub warnings: Vec<String>,
    pub items: Vec<FeedAuditItem>,
    pub next_cursor: Option<String>,
}
pub trait FeedOperationsStore: Send + Sync {
    /// 同一只读快照，最多 100 条元数据；不会联网或恢复状态。
    fn audit_feeds(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedAudit>>;
}
