//! 管理员显式分页核对，不读取文档正文或向量值。
use crate::{BoxFuture, StorageResult, VectorStore};
use personal_ai_domain::UserId;

#[derive(Clone, Debug)]
pub struct VectorReference {
    pub id: String,
    pub document_id: String,
}
#[derive(Clone, Debug)]
pub struct VectorReferencePage {
    pub items: Vec<VectorReference>,
    pub next_cursor: Option<String>,
}
pub trait VectorMaintenanceStore: VectorStore {
    fn list_references(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<VectorReferencePage>>;
}
pub trait DocumentPresenceStore: Send + Sync {
    fn document_exists(&self, owner: &UserId, document: &str)
    -> BoxFuture<'_, StorageResult<bool>>;
}
