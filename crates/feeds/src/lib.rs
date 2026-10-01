//! RSS 订阅采集的纯规划、受限解析及去重；无网络、存储写入或模型执行。
#![forbid(unsafe_code)]

pub mod parser;

use personal_ai_domain::UserId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::{Host, Url};
use uuid::Uuid;

pub const COLLECTION_VERSION: &str = "rss-manual-collection-v1";
pub const APPROVAL_TTL_MS: u64 = 300_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanError {
    InvalidSource,
    InvalidIdentity,
    InvalidRevision,
    InvalidTime,
    Disabled,
    OwnerMismatch,
    StaleSubscription,
    InvalidPlan,
    Expired,
    ConsentMismatch,
}

/// 由服务端从当前用户范围内的仓储加载，不能直接信任客户端传入的快照。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionSnapshot {
    pub user_id: String,
    pub subscription_id: String,
    pub revision: u64,
    pub source_url: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionPolicy {
    pub max_response_bytes: u32,
    pub max_items: u16,
    pub max_redirects: u8,
    pub timeout_ms: u32,
    pub fetch_article_pages: bool,
    pub fetch_enclosures: bool,
    pub call_models: bool,
}
impl Default for CollectionPolicy {
    fn default() -> Self {
        Self {
            max_response_bytes: 1_048_576,
            max_items: 100,
            max_redirects: 0,
            timeout_ms: 8_000,
            fetch_article_pages: false,
            fetch_enclosures: false,
            call_models: false,
        }
    }
}

/// 持久化前的不可变意图；摘要用于精确同意核对，不是认证签名。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionPlan {
    pub version: String,
    pub user_id: String,
    pub subscription_id: String,
    pub subscription_revision: u64,
    pub source_url: String,
    pub request_id: String,
    pub created_at_unix_ms: u64,
    pub approval_expires_at_unix_ms: u64,
    pub policy: CollectionPolicy,
}
impl CollectionPlan {
    /// # Errors
    /// 无法编码计划时返回 `InvalidPlan`。
    pub fn consent_digest(&self) -> Result<String, PlanError> {
        let bytes = serde_json::to_vec(self).map_err(|_| PlanError::InvalidPlan)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

/// 只检查和规范化 URL 语法；域名仍可能解析到内网，不能据此直接建立连接。
/// # Errors
/// 只允许 HTTPS、443 端口、无用户信息的完整 DNS 域名；拒绝 IP 字面量和本地域。
#[allow(clippy::case_sensitive_file_extension_comparisons)]
pub fn normalize_source(input: &str) -> Result<String, PlanError> {
    if input.len() > 2048 || input != input.trim() || input.chars().any(char::is_control) {
        return Err(PlanError::InvalidSource);
    }
    let mut url = Url::parse(input).map_err(|_| PlanError::InvalidSource)?;
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(PlanError::InvalidSource);
    }
    let Some(Host::Domain(host)) = url.host() else {
        return Err(PlanError::InvalidSource);
    };
    if host.len() > 253
        || !host.contains('.')
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        || ["localhost", "local", "internal", "invalid", "test", "onion"]
            .iter()
            .any(|suffix| host.ends_with(&format!(".{suffix}")))
    {
        return Err(PlanError::InvalidSource);
    }
    url.set_fragment(None);
    if url.as_str().len() > 2048 {
        return Err(PlanError::InvalidSource);
    }
    Ok(url.into())
}
fn identity(value: &str) -> Result<String, PlanError> {
    let id = Uuid::parse_str(value).map_err(|_| PlanError::InvalidIdentity)?;
    if id.is_nil() {
        return Err(PlanError::InvalidIdentity);
    }
    Ok(id.to_string())
}

/// 创建单次采集预览，不授予执行权。
/// # Errors
/// 拒绝无效身份、跨用户快照、停用订阅、零版本、非法来源和时间溢出。
pub fn plan_collection(
    user: &UserId,
    subscription: &SubscriptionSnapshot,
    request_id: &str,
    now_ms: u64,
) -> Result<CollectionPlan, PlanError> {
    let owner = identity(user.as_str())?;
    if owner != identity(&subscription.user_id)? {
        return Err(PlanError::OwnerMismatch);
    }
    if !subscription.enabled {
        return Err(PlanError::Disabled);
    }
    if subscription.revision == 0 {
        return Err(PlanError::InvalidRevision);
    }
    Ok(CollectionPlan {
        version: COLLECTION_VERSION.into(),
        user_id: owner,
        subscription_id: identity(&subscription.subscription_id)?,
        subscription_revision: subscription.revision,
        source_url: normalize_source(&subscription.source_url)?,
        request_id: identity(request_id)?,
        created_at_unix_ms: now_ms,
        approval_expires_at_unix_ms: now_ms
            .checked_add(APPROVAL_TTL_MS)
            .ok_or(PlanError::InvalidTime)?,
        policy: CollectionPolicy::default(),
    })
}

/// 核对用户对已保存预览的精确同意。调用方仍须在事务内一次性领取并再次复核订阅。
/// # Errors
/// 拒绝被修改的计划、过期/未来计划、不同意摘要、跨账户或已变化/停用的订阅。
pub fn validate_approval(
    user: &UserId,
    current: &SubscriptionSnapshot,
    saved: &CollectionPlan,
    accepted_digest: &str,
    now_ms: u64,
) -> Result<(), PlanError> {
    let expected = plan_collection(user, current, &saved.request_id, saved.created_at_unix_ms)?;
    if expected.user_id != saved.user_id {
        return Err(PlanError::OwnerMismatch);
    }
    if expected.subscription_id != saved.subscription_id
        || expected.subscription_revision != saved.subscription_revision
        || expected.source_url != saved.source_url
    {
        return Err(PlanError::StaleSubscription);
    }
    if expected != *saved {
        return Err(PlanError::InvalidPlan);
    }
    if now_ms < saved.created_at_unix_ms {
        return Err(PlanError::InvalidTime);
    }
    if now_ms >= saved.approval_expires_at_unix_ms {
        return Err(PlanError::Expired);
    }
    if accepted_digest != saved.consent_digest()? {
        return Err(PlanError::ConsentMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
