use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;

/// 服务端选择的非敏感配置标识，不得包含 API 密钥或地址中的凭据。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReplyConfiguration {
    pub model: String,
    pub revision: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReplyContext {
    pub system: String,
    pub user_messages: Vec<String>,
    pub first_sequence: i64,
    pub max_output_tokens: u32,
    pub configuration: ReplyConfiguration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyStatus {
    Queued,
    Dispatching,
    Succeeded,
    Failed,
    Unknown,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub request_id: String,
    pub revision: i64,
    pub status: ReplyStatus,
    /// 仅供内部执行者使用，不能直接作为 HTTP 响应序列化。
    pub context: Option<ReplyContext>,
    pub output: Option<String>,
}

pub enum ReplyOutcome {
    Succeeded(String),
    Failed,
    Unknown,
}

pub trait ReplyStore: Send + Sync {
    fn reserve_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        revision: i64,
        configuration: &ReplyConfiguration,
    ) -> BoxFuture<'_, StorageResult<Reply>>;
    fn get_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Reply>>;
    /// 只允许 queued 领取一次；提交失败或结果未知时不得调用模型。
    fn claim_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Reply>>;
    fn cancel_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Reply>>;
    fn finish_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        outcome: ReplyOutcome,
    ) -> BoxFuture<'_, StorageResult<Reply>>;
    /// 领取后超过固定期限的请求转为 unknown，不重新排队。
    fn expire_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Reply>>;
}
