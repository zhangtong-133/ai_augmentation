use crate::{PostgresStore, map_error};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult, vector_maintenance::DocumentPresenceStore,
};
use uuid::Uuid;
impl DocumentPresenceStore for PostgresStore {
    fn document_exists(
        &self,
        owner: &UserId,
        document: &str,
    ) -> BoxFuture<'_, StorageResult<bool>> {
        let parse = |s: &str| {
            Uuid::parse_str(s)
                .map_err(|_| StorageError::InvalidData("invalid document reference".into()))
        };
        let owner = parse(owner.as_str());
        let document = parse(document);
        Box::pin(async move {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM documents WHERE user_id=$1 AND id=$2)")
                .bind(owner?)
                .bind(document?)
                .fetch_one(&self.pool)
                .await
                .map_err(map_error)
        })
    }
}
