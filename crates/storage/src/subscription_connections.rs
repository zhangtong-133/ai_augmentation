//! Trusted runtime registration metadata; never store OAuth tokens here.
use crate::{BoxFuture, StorageResult, feeds::FeedPage};
use personal_ai_domain::UserId;
use serde::Serialize;

/// Only an authenticated runtime may construct this after checking identity, scopes and catalog.
/// This is deliberately not Deserialize: it is not an HTTP request body.
#[derive(Clone)]
pub struct VerifiedSubscriptionConnection {
    pub host_id: String,
    pub client_id: String,
    pub subject: String,
    pub label: String,
    pub models: Vec<String>,
    pub valid_until_unix_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SubscriptionConnection {
    pub id: String,
    pub label: String,
    pub revision: i64,
    pub status: String,
    pub models: Vec<String>,
    pub valid_until_unix_ms: i64,
}
impl SubscriptionConnection {
    #[must_use]
    pub fn configuration_version(&self) -> String {
        format!("connection-v{}", self.revision)
    }
}
pub trait SubscriptionConnectionStore: Send + Sync {
    /// `expected_revision=0` creates; exact input replay is idempotent. Never reassigns identity/owner.
    fn save_subscription_connection(
        &self,
        owner: &UserId,
        id: &str,
        expected_revision: i64,
        input: &VerifiedSubscriptionConnection,
    ) -> BoxFuture<'_, StorageResult<SubscriptionConnection>>;
    fn get_subscription_connection(
        &self,
        owner: &UserId,
        id: &str,
    ) -> BoxFuture<'_, StorageResult<SubscriptionConnection>>;
    fn list_subscription_connections(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedPage<SubscriptionConnection>>>;
    fn revoke_subscription_connection(
        &self,
        owner: &UserId,
        id: &str,
        expected_revision: i64,
    ) -> BoxFuture<'_, StorageResult<SubscriptionConnection>>;
}
