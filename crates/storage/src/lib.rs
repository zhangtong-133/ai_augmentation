#![forbid(unsafe_code)]
pub mod documents;
pub mod index_jobs;
pub mod long_memory;

use personal_ai_domain::{ConversationId, User, UserId};
use std::error::Error;
use std::fmt::{self, Display};
use std::future::Future;
use std::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type StorageResult<T> = Result<T, StorageError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageError {
    NotFound,
    Conflict(String),
    Unavailable(String),
    InvalidData(String),
}

impl Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("record not found"),
            Self::Conflict(message) => write!(formatter, "storage conflict: {message}"),
            Self::Unavailable(message) => write!(formatter, "storage unavailable: {message}"),
            Self::InvalidData(message) => write!(formatter, "invalid stored data: {message}"),
        }
    }
}

impl Error for StorageError {}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EmbeddingRecord {
    pub id: String,
    pub document_id: String,
    pub ordinal: usize,
    pub text: String,
    pub model: String,
    pub vector: Vec<f32>,
    pub source: String,
    pub content_type: String,
    pub created_at_unix_ms: u64,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VectorMatch {
    pub record: EmbeddingRecord,
    pub score: f32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryEntry {
    pub key: String,
    pub value: String,
    pub created_at_unix_ms: u64,
}

pub trait MetadataStore: Send + Sync {
    fn password_hash(&self, _email: &str) -> BoxFuture<'_, StorageResult<(UserId, String)>> {
        Box::pin(async { Err(StorageError::NotFound) })
    }
    fn set_password(&self, _id: &UserId, _hash: &str) -> BoxFuture<'_, StorageResult<()>> {
        Box::pin(async {
            Err(StorageError::Unavailable(
                "authentication not supported".into(),
            ))
        })
    }
    fn create_session(
        &self,
        _id: &UserId,
        _digest: &str,
        _credential_hash: &str,
    ) -> BoxFuture<'_, StorageResult<()>> {
        Box::pin(async {
            Err(StorageError::Unavailable(
                "authentication not supported".into(),
            ))
        })
    }
    fn session_user(&self, _digest: &str) -> BoxFuture<'_, StorageResult<User>> {
        Box::pin(async { Err(StorageError::NotFound) })
    }
    fn delete_session(&self, _digest: &str) -> BoxFuture<'_, StorageResult<()>> {
        Box::pin(async {
            Err(StorageError::Unavailable(
                "authentication not supported".into(),
            ))
        })
    }
    fn health(&self) -> BoxFuture<'_, StorageResult<()>>;
    fn get_user(&self, id: &UserId) -> BoxFuture<'_, StorageResult<User>>;
    fn save_user(&self, user: &User) -> BoxFuture<'_, StorageResult<()>>;
}

pub trait VectorStore: Send + Sync {
    fn insert_embeddings(
        &self,
        owner: &UserId,
        records: &[EmbeddingRecord],
    ) -> BoxFuture<'_, StorageResult<()>>;

    fn similar_search(
        &self,
        owner: &UserId,
        query: &[f32],
        limit: usize,
    ) -> BoxFuture<'_, StorageResult<Vec<VectorMatch>>>;

    fn remove(&self, owner: &UserId, ids: &[String]) -> BoxFuture<'_, StorageResult<()>>;
}

#[derive(Clone, Debug)]
pub struct ObjectInfo {
    pub key: String,
    pub modified_unix_ms: i64,
}

pub trait ObjectStorage: Send + Sync {
    /// 按键名升序列出 users/ 下、游标之后的对象，最多 100 项。
    fn list_originals(&self, _after: &str) -> BoxFuture<'_, StorageResult<Vec<ObjectInfo>>> {
        Box::pin(async {
            Err(StorageError::Unavailable(
                "object listing unsupported".into(),
            ))
        })
    }
    fn head(&self, _key: &str) -> BoxFuture<'_, StorageResult<ObjectInfo>> {
        Box::pin(async {
            Err(StorageError::Unavailable(
                "object metadata unsupported".into(),
            ))
        })
    }
    fn put(
        &self,
        key: &str,
        bytes: &[u8],
        content_type: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<()>>;

    fn get(&self, key: &str) -> BoxFuture<'_, StorageResult<Vec<u8>>>;
    fn delete(&self, key: &str) -> BoxFuture<'_, StorageResult<()>>;
}

pub trait MemoryStore: Send + Sync {
    fn append(
        &self,
        user_id: &UserId,
        conversation_id: &ConversationId,
        entry: &MemoryEntry,
    ) -> BoxFuture<'_, StorageResult<()>>;

    fn recent(
        &self,
        user_id: &UserId,
        conversation_id: &ConversationId,
        limit: usize,
    ) -> BoxFuture<'_, StorageResult<Vec<MemoryEntry>>>;
}
