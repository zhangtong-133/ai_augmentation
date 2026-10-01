//! 确定性 RSS 日报纯规划；输入必须由服务端从当前用户的仓储加载。
use crate::{identity, normalize_source};
use personal_ai_domain::UserId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const BRIEF_VERSION: &str = "rss-rule-brief-v1";
pub const DAY_MS: u64 = 86_400_000;
pub const MAX_CANDIDATES: usize = 500;
pub const MAX_ITEMS: usize = 20;
pub const MAX_PER_SUBSCRIPTION: usize = 3;
const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BriefError {
    InvalidIdentity,
    OwnerMismatch,
    InvalidWindow,
    InvalidPreferences,
    InvalidEntry,
    TooLarge,
    ConflictingDuplicate,
    Encoding,
}

/// 私有内容不实现 Debug；不从 HTTP 反序列化。enabled/deleted 来自同一仓储快照。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BriefCandidate {
    pub user_id: String,
    pub subscription_id: String,
    pub entry_key: String,
    pub enabled: bool,
    pub deleted: bool,
    pub title: String,
    pub summary: String,
    pub link: Option<String>,
    pub published_at: Option<String>,
    pub first_seen_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub last_seen_unix_ms: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeywordMatch {
    pub keyword: String,
    pub in_title: bool,
    pub points: u16,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BriefItem {
    pub entry: BriefCandidate,
    pub score: u16,
    pub freshness_points: u16,
    pub matches: Vec<KeywordMatch>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BriefPlan {
    pub version: String,
    pub user_id: String,
    pub request_id: String,
    pub day_start_unix_ms: u64,
    pub day_end_unix_ms: u64,
    pub as_of_unix_ms: u64,
    pub keywords: Vec<String>,
    pub input_digest: String,
    pub candidate_count: usize,
    pub eligible_count: usize,
    pub duplicate_count: usize,
    pub omitted_count: usize,
    pub items: Vec<BriefItem>,
}
impl BriefPlan {
    /// 绑定版本、用户、窗口、偏好、输入及输出，不代表联网或模型授权。
    /// # Errors
    /// 序列化失败时拒绝生成摘要。
    pub fn digest(&self) -> Result<String, BriefError> {
        hash(self)
    }
}
fn hash(value: &impl Serialize) -> Result<String, BriefError> {
    let bytes = serde_json::to_vec(value).map_err(|_| BriefError::Encoding)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn canonical_id(value: &str) -> Result<String, BriefError> {
    identity(value).map_err(|_| BriefError::InvalidIdentity)
}
/// 规范化服务端保存的显式关键词偏好。
/// # Errors
/// 拒绝空词、重复、控制字符和超限偏好。
pub fn normalize_keywords(input: &[String]) -> Result<Vec<String>, BriefError> {
    if input.len() > 5 {
        return Err(BriefError::InvalidPreferences);
    }
    let mut terms = BTreeSet::new();
    for value in input {
        if value.len() > 256 || value.chars().any(char::is_control) || value.chars().count() > 64 {
            return Err(BriefError::InvalidPreferences);
        }
        let term = value.trim().to_lowercase();
        if term.is_empty() || term.chars().count() > 64 || !terms.insert(term) {
            return Err(BriefError::InvalidPreferences);
        }
    }
    Ok(terms.into_iter().collect())
}
fn normalize(
    entry: &BriefCandidate,
    owner: &str,
    as_of: u64,
) -> Result<BriefCandidate, BriefError> {
    if canonical_id(&entry.user_id)? != owner {
        return Err(BriefError::OwnerMismatch);
    }
    let key = entry
        .entry_key
        .strip_prefix("guid:")
        .or_else(|| entry.entry_key.strip_prefix("link:"))
        .ok_or(BriefError::InvalidEntry)?;
    if key.len() != 64
        || !key
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        || entry.title.chars().count() > 512
        || entry.summary.chars().count() > 8192
        || (entry.title.trim().is_empty() && entry.summary.trim().is_empty())
        || entry
            .published_at
            .as_ref()
            .is_some_and(|s| s.chars().count() > 256)
        || entry.first_seen_unix_ms == 0
        || entry.first_seen_unix_ms > entry.updated_at_unix_ms
        || entry.updated_at_unix_ms > entry.last_seen_unix_ms
        || entry.last_seen_unix_ms > as_of
    {
        return Err(BriefError::InvalidEntry);
    }
    let mut saved = entry.clone();
    saved.user_id = owner.into();
    saved.subscription_id = canonical_id(&entry.subscription_id)?;
    saved.link = entry
        .link
        .as_deref()
        .map(normalize_source)
        .transpose()
        .map_err(|_| BriefError::InvalidEntry)?;
    Ok(saved)
}
fn score(entry: BriefCandidate, keywords: &[String], as_of: u64) -> BriefItem {
    let title = entry.title.to_lowercase();
    let summary = entry.summary.to_lowercase();
    let matches: Vec<_> = keywords
        .iter()
        .filter_map(|keyword| {
            let in_title = title.contains(keyword);
            (in_title || summary.contains(keyword)).then(|| KeywordMatch {
                keyword: keyword.clone(),
                in_title,
                points: if in_title { 12 } else { 6 },
            })
        })
        .collect();
    let age = as_of - entry.first_seen_unix_ms;
    let freshness_points = if age <= 6 * 3_600_000 {
        40
    } else if age <= 12 * 3_600_000 {
        30
    } else {
        20
    };
    BriefItem {
        entry,
        score: freshness_points + matches.iter().map(|m| m.points).sum::<u16>(),
        freshness_points,
        matches,
    }
}

/// 对显式 UTC 日窗口中的已保存条目生成不可变候选日报，无 I/O 或自动授权。
/// # Errors
/// 拒绝跨用户、非 UTC 日界限、超过截点的条目时间、超限输入及冲突的复合条目键。
/// 此函数不能识别仓储查询遗漏；调用方必须提交完整的有界候选快照，不能静默截断。
pub fn plan_brief(
    user: &UserId,
    request_id: &str,
    day_start: u64,
    as_of: u64,
    keywords: &[String],
    candidates: &[BriefCandidate],
) -> Result<BriefPlan, BriefError> {
    let owner = canonical_id(user.as_str())?;
    let request = canonical_id(request_id)?;
    let day_end = day_start
        .checked_add(DAY_MS)
        .ok_or(BriefError::InvalidWindow)?;
    if !day_start.is_multiple_of(DAY_MS)
        || day_end > i64::MAX.cast_unsigned()
        || as_of < day_start
        || as_of > day_end
    {
        return Err(BriefError::InvalidWindow);
    }
    if candidates.len() > MAX_CANDIDATES {
        return Err(BriefError::TooLarge);
    }
    let keywords = normalize_keywords(keywords)?;
    let mut bytes = 0_usize;
    let mut unique = BTreeMap::new();
    for candidate in candidates {
        for text in [
            &candidate.user_id,
            &candidate.subscription_id,
            &candidate.entry_key,
            &candidate.title,
            &candidate.summary,
            candidate.link.as_deref().unwrap_or(""),
            candidate.published_at.as_deref().unwrap_or(""),
        ] {
            bytes = bytes.checked_add(text.len()).ok_or(BriefError::TooLarge)?;
        }
        if bytes > MAX_INPUT_BYTES {
            return Err(BriefError::TooLarge);
        }
        let saved = normalize(candidate, &owner, as_of)?;
        let key = (saved.subscription_id.clone(), saved.entry_key.clone());
        if let Some(old) = unique.insert(key, saved.clone())
            && old != saved
        {
            return Err(BriefError::ConflictingDuplicate);
        }
    }
    let input_digest = hash(&unique.values().collect::<Vec<_>>())?;
    let candidate_count = unique.len();
    let mut ranked: Vec<_> = unique
        .into_values()
        .filter(|e| {
            e.enabled
                && !e.deleted
                && e.first_seen_unix_ms >= day_start
                && e.first_seen_unix_ms < day_end
        })
        .map(|e| score(e, &keywords, as_of))
        .collect();
    let eligible_count = ranked.len();
    ranked.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.entry.first_seen_unix_ms.cmp(&a.entry.first_seen_unix_ms))
            .then_with(|| a.entry.subscription_id.cmp(&b.entry.subscription_id))
            .then_with(|| a.entry.entry_key.cmp(&b.entry.entry_key))
    });
    let mut selected = Vec::new();
    let mut per_subscription = BTreeMap::new();
    let mut all_content = BTreeSet::new();
    let mut selected_content = BTreeSet::new();
    for item in ranked {
        // 保守去重：规范链接、标题和摘要全相同；不做语义或相似内容合并。
        let content = hash(&(&item.entry.link, &item.entry.title, &item.entry.summary))?;
        all_content.insert(content.clone());
        if selected_content.contains(&content) {
            continue;
        }
        let count = per_subscription
            .entry(item.entry.subscription_id.clone())
            .or_insert(0);
        if selected.len() < MAX_ITEMS && *count < MAX_PER_SUBSCRIPTION {
            *count += 1;
            selected_content.insert(content);
            selected.push(item);
        }
    }
    let duplicate_count = eligible_count - all_content.len();
    Ok(BriefPlan {
        version: BRIEF_VERSION.into(),
        user_id: owner,
        request_id: request,
        day_start_unix_ms: day_start,
        day_end_unix_ms: day_end,
        as_of_unix_ms: as_of,
        keywords,
        input_digest,
        candidate_count,
        eligible_count,
        duplicate_count,
        omitted_count: eligible_count - duplicate_count - selected.len(),
        items: selected,
    })
}

#[cfg(test)]
mod tests;
