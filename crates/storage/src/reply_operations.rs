//! 管理员内部的配置查询和金额审计端口，不接入用户 HTTP 参数。
use crate::{
    BoxFuture, StorageError, StorageResult, replies::ReplyConfiguration, reply_budgets::ReplyBudget,
};
use personal_ai_domain::UserId;

pub struct ReplyConfigurationRecord {
    pub configuration: ReplyConfiguration,
    pub budget: ReplyBudget,
    pub valid_until_unix_ms: i64,
    pub created_at_unix_ms: i64,
    pub disabled_at_unix_ms: Option<i64>,
    pub active: bool,
}

pub struct ReplyConfigurationPage {
    pub items: Vec<ReplyConfigurationRecord>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ReplySettlementCounts {
    pub requests: i64,
    pub pending: i64,
    pub cancelled_before_dispatch: i64,
    pub verified: i64,
    pub retained: i64,
    pub usage_exceeded: i64,
}

/// 累计值可超过 i64，使用精确十进制字符串；retained 不是供应商实际账单。
#[derive(Debug, serde::Serialize)]
pub struct ReplyLedgerTotals {
    pub reserved_micro: String,
    pub pending_micro: String,
    pub settled_micro: String,
    pub retained_micro: String,
    pub refunded_micro: String,
    pub expected_occupied_micro: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ReplyAuditReceipt {
    pub request_kind: String,
    pub conversation_id: String,
    pub request_id: String,
    pub model: String,
    pub configuration_revision: String,
    pub reserved_micro: String,
    pub charged_micro: Option<String>,
    pub settlement: Option<String>,
    pub created_at_unix_ms: String,
    pub settled_at_unix_ms: Option<String>,
    pub conversation_deleted: bool,
    pub reply_status: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ReplyMoneyAudit {
    pub user_id: String,
    pub day: String,
    pub currency: String,
    pub snapshot_at_unix_ms: String,
    pub counts: ReplySettlementCounts,
    pub totals: ReplyLedgerTotals,
    pub ledger_occupied_micro: Option<String>,
    /// 日账本减去全部凭据的应占用金额。缺失账本按 0 计算差额，另由 consistent 报错。
    pub difference_micro: String,
    pub consistent: bool,
    pub items: Vec<ReplyAuditReceipt>,
    /// conversation UUID/request UUID，最多返回 100 项；汇总始终覆盖整个用户日账本。
    pub next_cursor: Option<String>,
}

pub trait ReplyOperationsStore: Send + Sync {
    fn list_reply_configurations(
        &self,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ReplyConfigurationPage>>;

    fn get_reply_configuration(
        &self,
        revision: &str,
    ) -> BoxFuture<'_, StorageResult<ReplyConfigurationRecord>>;

    /// 同一只读快照核对日账本、全部结算凭据和一页明细；包含已删除对话。
    fn audit_reply_money(
        &self,
        owner: &UserId,
        day: &str,
        currency: &str,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ReplyMoneyAudit>>;
}

/// # Errors
/// 拒绝空白、超长或含 NUL 的内部版本标识。
pub fn validate_reply_revision(revision: &str) -> StorageResult<()> {
    if revision.trim().is_empty() || revision.len() > 128 || revision.contains('\0') {
        return Err(StorageError::InvalidData(
            "invalid reply configuration revision".into(),
        ));
    }
    Ok(())
}

/// # Errors
/// 日期必须是有效的 YYYY-MM-DD，币种必须是三个大写 ASCII 字母。
pub fn validate_reply_audit_scope(day: &str, currency: &str) -> StorageResult<()> {
    crate::dates::validate_utc_day(day)?;
    if currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(StorageError::InvalidData(
            "invalid reply audit day or currency".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_rejects_ambiguous_dates_invalid_calendar_days_and_currency() {
        for day in [
            "2026-09-30",
            "2000-02-29",
            "2024-02-29",
            "0001-01-01",
            "9999-12-31",
        ] {
            assert!(validate_reply_audit_scope(day, "USD").is_ok());
        }
        for day in [
            "2026-02-29",
            "1900-02-29",
            "0000-01-01",
            "2026-09-31",
            "2026-9-30",
            "2026-00-01",
            "2026-13-01",
            "2026-01-00",
            "2026-+1-01",
            "2026-01-01; SELECT 1",
            "２０２６-０１-０１",
        ] {
            assert!(validate_reply_audit_scope(day, "USD").is_err());
        }
        for currency in ["usd", "US", "USDD", "人民币", "US\0"] {
            assert!(validate_reply_audit_scope("2026-09-30", currency).is_err());
        }
    }
}
