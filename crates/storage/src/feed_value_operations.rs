//! RSS 评分只读元数据，不查询分享正文、结果、报价或执行令牌。
use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;
use std::collections::BTreeMap;

#[derive(Debug, serde::Serialize)]
pub struct FeedValueAuditItem {
    pub request_id: String,
    pub status: String,
    pub created_at_unix_ms: String,
    pub expires_at_unix_ms: String,
    pub approved_at_unix_ms: Option<String>,
    pub dispatch_deadline_unix_ms: Option<String>,
    pub sent_at_unix_ms: Option<String>,
    pub issues: Vec<String>,
}
#[derive(Debug, serde::Serialize)]
pub struct FeedValueAudit {
    pub user_id: String,
    pub snapshot_at_unix_ms: String,
    pub counts: BTreeMap<String, i64>,
    pub remaining_previews_today: i64,
    pub remaining_records: i64,
    pub consistent: bool,
    pub issues: Vec<String>,
    pub warnings: Vec<String>,
    pub items: Vec<FeedValueAuditItem>,
    pub next_cursor: Option<String>,
}
pub trait FeedValueOperationsStore: Send + Sync {
    /// 同一只读快照；汇总不受游标影响，每页最多 100 条，不恢复或重发请求。
    fn audit_feed_values(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedValueAudit>>;
}
