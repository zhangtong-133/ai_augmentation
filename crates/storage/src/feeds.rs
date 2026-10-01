//! RSS 私有订阅、单次采集授权及结果仓储；没有隐式网络执行。
use personal_ai_domain::UserId;
use personal_ai_feeds::{CollectionPlan, SubscriptionSnapshot};
use serde::Serialize;

use crate::{BoxFuture, StorageResult};

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct Subscription {
    pub snapshot: SubscriptionSnapshot,
    pub name: String,
    pub deleted: bool,
}

#[derive(Clone)]
pub struct SubscriptionInput {
    pub name: String,
    pub source_url: String,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionStatus {
    Draft,
    Running,
    Succeeded,
    Failed,
    Unknown,
    Cancelled,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct EntryCounts {
    pub inserted: u32,
    pub updated: u32,
    pub unchanged: u32,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct Collection {
    pub plan: CollectionPlan,
    pub digest: String,
    pub status: CollectionStatus,
    pub claimed_at_unix_ms: Option<i64>,
    pub deadline_unix_ms: Option<i64>,
    pub finished_at_unix_ms: Option<i64>,
    pub reason: Option<String>,
    pub counts: EntryCounts,
}

/// 仅内部执行器接收，不从 HTTP 反序列化；同一请求只会成功返回一次凭据。
#[derive(Clone)]
pub struct CollectionClaim {
    pub owner: UserId,
    pub request_id: String,
    pub claim_id: String,
    pub plan: CollectionPlan,
}

#[derive(Clone, Copy)]
pub enum CollectionFailure {
    Transport,
    Unknown,
}

/// 原始响应只用于离线校验，永不保存；不可传入可修改的解析条目绕过校验。
pub enum CollectionOutcome {
    Response(Vec<u8>),
    Failure(CollectionFailure),
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct StoredEntry {
    pub entry_key: String,
    pub title: String,
    pub summary: String,
    pub link: Option<String>,
    pub published_at: Option<String>,
    pub content_digest: String,
    pub first_seen_unix_ms: i64,
    pub updated_at_unix_ms: i64,
    pub last_seen_unix_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CollectionAudit {
    pub event: String,
    pub at_unix_ms: i64,
    pub reason: Option<String>,
    pub counts: EntryCounts,
}

#[derive(Clone, Serialize)]
pub struct FeedPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

pub trait FeedStore: Send + Sync {
    fn create_subscription(
        &self,
        owner: &UserId,
        id: &str,
        input: &SubscriptionInput,
    ) -> BoxFuture<'_, StorageResult<Subscription>>;
    fn update_subscription(
        &self,
        owner: &UserId,
        id: &str,
        revision: u64,
        input: &SubscriptionInput,
    ) -> BoxFuture<'_, StorageResult<Subscription>>;
    fn delete_subscription(
        &self,
        owner: &UserId,
        id: &str,
        revision: u64,
    ) -> BoxFuture<'_, StorageResult<()>>;
    fn get_subscription(
        &self,
        owner: &UserId,
        id: &str,
    ) -> BoxFuture<'_, StorageResult<Subscription>>;
    fn list_subscriptions(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedPage<Subscription>>>;
    /// 服务端加载当前订阅并生成计划；相同请求 ID 只返回原预览，不刷新有效期。
    fn preview_collection(
        &self,
        owner: &UserId,
        subscription: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Collection>>;
    fn get_collection(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Collection>>;
    fn list_collections(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedPage<Collection>>>;
    /// 精确同意与领取在同一事务；重复调用冲突，不再次返回执行凭据。
    fn claim_collection(
        &self,
        owner: &UserId,
        request: &str,
        accepted_digest: &str,
    ) -> BoxFuture<'_, StorageResult<CollectionClaim>>;
    fn cancel_collection(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Collection>>;
    fn finish_collection(
        &self,
        claim: &CollectionClaim,
        outcome: CollectionOutcome,
    ) -> BoxFuture<'_, StorageResult<Collection>>;
    /// 仅将超过执行期限的 running 标记为 unknown；不退款、不重派。
    fn recover_collection(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Collection>>;
    fn list_feed_entries(
        &self,
        owner: &UserId,
        subscription: &str,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedPage<StoredEntry>>>;
    fn collection_audit(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Vec<CollectionAudit>>>;
}
