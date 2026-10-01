//! MCP 凭据只读元数据核对，不读取宿主名称或认证摘要。
use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;

#[derive(Debug, serde::Serialize)]
pub struct McpAuditCounts {
    pub total: i64,
    pub active: i64,
    pub expired: i64,
    pub revoked: i64,
    pub issued_last_24h: i64,
    /// 有效凭据与最近 24 小时签发记录的并集，与签发入口一致。
    pub quota_used: i64,
    pub inconsistent: i64,
}
#[derive(Debug, serde::Serialize)]
pub struct McpAuditItem {
    pub id: String,
    pub status: String,
    pub created_at_unix_ms: String,
    pub expires_at_unix_ms: String,
    pub revoked_at_unix_ms: Option<String>,
    pub issues: Vec<String>,
}
#[derive(Debug, serde::Serialize)]
pub struct McpAudit {
    pub user_id: String,
    pub snapshot_at_unix_ms: String,
    pub counts: McpAuditCounts,
    pub quota_limit: i64,
    pub remaining_issuance: i64,
    pub consistent: bool,
    pub issues: Vec<String>,
    pub items: Vec<McpAuditItem>,
    pub next_cursor: Option<String>,
}
pub trait McpOperationsStore: Send + Sync {
    /// 汇总和当前页使用同一个只读快照；每页至多 100 条。
    fn audit_mcp_credentials(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<McpAudit>>;
}
