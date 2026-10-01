//! 学习数据只读元数据核对，不读取名称、分数、计划、任务或结果正文。
use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;
use std::collections::BTreeMap;

#[derive(Debug, serde::Serialize)]
pub struct LearningQuota {
    pub used: i64,
    pub limit: i64,
    pub remaining: i64,
}
#[derive(Debug, serde::Serialize)]
pub struct LearningAuditItem {
    pub request_id: String,
    pub status: String,
    pub snapshot_revision: String,
    pub created_at_unix_ms: String,
    pub issues: Vec<String>,
}
#[derive(Debug, serde::Serialize)]
pub struct LearningAudit {
    pub user_id: String,
    pub snapshot_at_unix_ms: String,
    pub revision: String,
    /// 全用户汇总，不受计划分页影响。
    pub counts: BTreeMap<String, i64>,
    pub quotas: BTreeMap<String, LearningQuota>,
    pub consistent: bool,
    pub issues: Vec<String>,
    pub warnings: Vec<String>,
    pub items: Vec<LearningAuditItem>,
    pub next_cursor: Option<String>,
}
pub trait LearningOperationsStore: Send + Sync {
    /// 同一个只读快照内汇总并返回至多 100 条计划元数据，不迁移或修复数据。
    fn audit_learning(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<LearningAudit>>;
}
