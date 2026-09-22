use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub created_at_unix_ms: i64,
}

pub trait ConversationStore: Send + Sync {
    fn create_conversation(
        &self,
        owner: &UserId,
        request_id: &str,
        title: &str,
    ) -> BoxFuture<'_, StorageResult<Conversation>>;
    fn list_conversations(&self, owner: &UserId)
    -> BoxFuture<'_, StorageResult<Vec<Conversation>>>;
    fn get_conversation(
        &self,
        owner: &UserId,
        id: &str,
    ) -> BoxFuture<'_, StorageResult<Conversation>>;
    fn delete_conversation(&self, owner: &UserId, id: &str) -> BoxFuture<'_, StorageResult<()>>;
}
