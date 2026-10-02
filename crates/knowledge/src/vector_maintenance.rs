//! 显式维护：调用方须确保数据库/集合匹配并暂停写入及等待在途索引结束。
use personal_ai_domain::UserId;
use personal_ai_storage::{
    StorageResult,
    vector_maintenance::{DocumentPresenceStore, VectorMaintenanceStore},
};

#[derive(Debug, serde::Serialize)]
pub struct VectorCleanupReport {
    pub apply: bool,
    pub scanned: usize,
    pub eligible: usize,
    pub changed: usize,
    pub next_cursor: Option<String>,
}
/// 核对一页无正文引用；执行前再次检查文档是否存在。错误中止，重跑安全。
///
/// # Errors
/// 任一存储不可用或响应无效时返回错误，不将数据库错误视为文档缺失。
pub async fn collect_vectors(
    documents: &dyn DocumentPresenceStore,
    vectors: &dyn VectorMaintenanceStore,
    owner: &UserId,
    after: Option<&str>,
    apply: bool,
) -> StorageResult<VectorCleanupReport> {
    let page = vectors.list_references(owner, after).await?;
    let mut eligible = Vec::new();
    for item in &page.items {
        if !documents.document_exists(owner, &item.document_id).await? {
            eligible.push(item);
        }
    }
    let mut report = VectorCleanupReport {
        apply,
        scanned: page.items.len(),
        eligible: eligible.len(),
        changed: 0,
        next_cursor: page.next_cursor,
    };
    if apply {
        for item in eligible {
            if !documents.document_exists(owner, &item.document_id).await? {
                vectors
                    .remove(owner, std::slice::from_ref(&item.id))
                    .await?;
                report.changed += 1;
            }
        }
    }
    Ok(report)
}
