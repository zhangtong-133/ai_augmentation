//! 固定合成 RSS 的质量基线，独立于协议正确性和用户评分状态。
use crate::{
    feed_value::{Score, ValueError, ValueScoringPlan, plan_value_scoring},
    feed_value_local::{
        LOCAL_VALUE_PROFILE, decode_for_profile, request_digest, request_for_profile,
    },
};
use personal_ai_domain::UserId;
use personal_ai_feeds::brief::{BriefCandidate, DAY_MS};
use personal_ai_llm::ChatRequest;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
const CORPUS: &str = include_str!("feed_value_quality/corpus.json");
const CHALLENGE: &str = include_str!("feed_value_quality/challenge.json");
pub const CHALLENGE_VERSION: &str = "rss-challenge-v1";
pub const QUALITY_VERSION: &str = "rss-quality-v1";
const OWNER: &str = "11111111-1111-4111-8111-111111111111";
const START: u64 = 20_000 * DAY_MS;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    version: String,
    cases: Vec<CaseInput>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseInput {
    id: String,
    keywords: Vec<String>,
    entries: Vec<Entry>,
    checks: Vec<Check>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    key: String,
    title: String,
    summary: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Check {
    Minimum {
        item: String,
        value: u8,
    },
    Ceiling {
        item: String,
        value: u8,
    },
    Abstain {
        item: String,
    },
    Prefer {
        higher: String,
        lower: String,
        margin: u8,
    },
    NoCanary {
        text: String,
    },
}
pub struct QualityCase {
    id: String,
    corpus_sha256: String,
    profile: &'static str,
    quality_version: &'static str,
    plan: ValueScoringPlan,
    labels: BTreeMap<usize, String>,
    checks: Vec<Check>,
}
#[derive(Serialize)]
pub struct CaseManifest {
    pub id: String,
    pub corpus_sha256: String,
    pub input_digest: String,
    pub request_sha256: String,
    pub execution_profile: &'static str,
    pub quality_version: &'static str,
    pub prompt_bytes: usize,
    pub checks: Vec<Check>,
    pub items: Vec<String>,
}
#[derive(Serialize)]
pub struct QualityResult {
    pub manifest: CaseManifest,
    pub protocol_valid: bool,
    pub quality_pass: bool,
    pub checks: Vec<CheckResult>,
    pub scores: BTreeMap<String, Option<u8>>,
}
#[derive(Serialize)]
pub struct CheckResult {
    pub index: usize,
    pub passed: bool,
}
fn key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 40
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
impl Check {
    fn valid(&self, labels: &BTreeSet<&str>) -> bool {
        match self {
            Self::Minimum { item, value } | Self::Ceiling { item, value } => {
                labels.contains(item.as_str()) && *value <= 100
            }
            Self::Abstain { item } => labels.contains(item.as_str()),
            Self::Prefer {
                higher,
                lower,
                margin,
            } => {
                higher != lower
                    && *margin > 0
                    && *margin <= 100
                    && labels.contains(higher.as_str())
                    && labels.contains(lower.as_str())
            }
            Self::NoCanary { text } => !text.is_empty() && text.len() <= 80 && text.is_ascii(),
        }
    }
    fn passes(&self, scores: &BTreeMap<String, Option<u8>>, raw: &[Score]) -> bool {
        match self {
            Self::Minimum { item, value } => scores[item].is_some_and(|score| score >= *value),
            Self::Ceiling { item, value } => scores[item].is_none_or(|score| score <= *value),
            Self::Abstain { item } => scores[item].is_none(),
            Self::Prefer {
                higher,
                lower,
                margin,
            } => scores[higher].is_some_and(|score| {
                scores[lower]
                    .is_none_or(|other| i16::from(score) - i16::from(other) >= i16::from(*margin))
            }),
            Self::NoCanary { text } => raw.iter().all(|s| !s.reason.contains(text)),
        }
    }
}
/// 固定语料包含人工选择的阈值；通过不代表用户材料质量。
/// # Errors
/// 无效语料、丢失检查目标或超过本地预算时拒绝运行。
pub fn cases() -> Result<Vec<QualityCase>, ValueError> {
    parse(CORPUS)
}
/// # Errors
/// 未知 profile 或不合法语料时拒绝。
pub fn cases_for_profile(profile: &str) -> Result<Vec<QualityCase>, ValueError> {
    cases_for_suite("baseline", profile)
}
/// 独立挑战集不替换或修改原始基线。
/// # Errors
/// 未知套件/profile、不合法语料或预算超限时拒绝。
pub fn cases_for_suite(suite: &str, profile: &str) -> Result<Vec<QualityCase>, ValueError> {
    let (input, version) = match suite {
        "baseline" => (CORPUS, QUALITY_VERSION),
        "challenge" => (CHALLENGE, CHALLENGE_VERSION),
        _ => return Err(ValueError::InvalidSnapshot),
    };
    let profile = match profile {
        "local-rss-v1" => "local-rss-v1",
        "local-rss-v2" => "local-rss-v2",
        _ => return Err(ValueError::InvalidSnapshot),
    };
    let mut all = parse_version(input, version)?;
    for case in &mut all {
        case.profile = profile;
        case.request()?;
    }
    Ok(all)
}
fn parse(input: &str) -> Result<Vec<QualityCase>, ValueError> {
    parse_version(input, QUALITY_VERSION)
}
fn parse_version(input: &str, version: &'static str) -> Result<Vec<QualityCase>, ValueError> {
    let corpus_sha256 = format!("{:x}", Sha256::digest(input));
    let corpus: Corpus = serde_json::from_str(input).map_err(|_| ValueError::InvalidSnapshot)?;
    if corpus.version != version || corpus.cases.is_empty() || corpus.cases.len() > 8 {
        return Err(ValueError::InvalidSnapshot);
    }
    let mut ids = BTreeSet::new();
    corpus
        .cases
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            if !key(&c.id)
                || !ids.insert(c.id.clone())
                || c.entries.is_empty()
                || c.entries.len() > 8
                || c.checks.is_empty()
                || c.checks.len() > 16
            {
                return Err(ValueError::InvalidSnapshot);
            }
            let labels: BTreeSet<_> = c.entries.iter().map(|e| e.key.as_str()).collect();
            if labels.len() != c.entries.len()
                || labels.iter().any(|s| !key(s))
                || c.checks.iter().any(|check| !check.valid(&labels))
            {
                return Err(ValueError::InvalidSnapshot);
            }
            let candidates: Vec<_> = c
                .entries
                .iter()
                .enumerate()
                .map(|(j, e)| BriefCandidate {
                    user_id: OWNER.into(),
                    subscription_id: format!("33333333-3333-4333-8333-{j:012}"),
                    entry_key: format!("guid:{j:064x}"),
                    enabled: true,
                    deleted: false,
                    title: e.title.clone(),
                    summary: e.summary.clone(),
                    link: None,
                    published_at: None,
                    first_seen_unix_ms: START + 1,
                    updated_at_unix_ms: START + 1,
                    last_seen_unix_ms: START + 1,
                })
                .collect();
            let plan = plan_value_scoring(
                &UserId::new(OWNER),
                &format!("22222222-2222-4222-8222-{i:012}"),
                START,
                START + 1000,
                &c.keywords,
                &candidates,
            )?
            .ok_or(ValueError::InvalidSnapshot)?;
            request_for_profile(&plan, LOCAL_VALUE_PROFILE)?;
            if plan.brief().items.len() != c.entries.len() {
                return Err(ValueError::InvalidSnapshot);
            }
            let labels = plan
                .brief()
                .items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let label = candidates
                        .iter()
                        .position(|entry| entry.entry_key == item.entry.entry_key)
                        .ok_or(ValueError::InvalidSnapshot)?;
                    Ok((index + 1, c.entries[label].key.clone()))
                })
                .collect::<Result<_, ValueError>>()?;
            Ok(QualityCase {
                id: c.id,
                corpus_sha256: corpus_sha256.clone(),
                profile: LOCAL_VALUE_PROFILE,
                quality_version: version,
                plan,
                labels,
                checks: c.checks,
            })
        })
        .collect()
}
impl QualityCase {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
    /// # Errors
    /// 精确请求超出本地预算时拒绝发送。
    pub fn request(&self) -> Result<ChatRequest, ValueError> {
        request_for_profile(&self.plan, self.profile)
    }
    /// # Errors
    /// 不能生成精确本地请求时失败。
    pub fn manifest(&self) -> Result<CaseManifest, ValueError> {
        let request = self.request()?;
        Ok(CaseManifest {
            id: self.id.clone(),
            corpus_sha256: self.corpus_sha256.clone(),
            input_digest: self.plan.digest().into(),
            request_sha256: request_digest(&self.plan, self.profile)?,
            execution_profile: self.profile,
            quality_version: self.quality_version,
            prompt_bytes: request.messages.iter().map(|m| m.content.len()).sum(),
            checks: self.checks.clone(),
            items: self.labels.values().cloned().collect(),
        })
    }
    /// 先执行业务完整评分校验，再分别记录质量条件；不输出理由或正文。
    /// # Errors
    /// 截断、非法或缺失条目等协议失败不能成为质量结果。
    pub fn evaluate(&self, output: &[u8]) -> Result<QualityResult, ValueError> {
        let raw = decode_for_profile(&self.plan, self.profile, output)?;
        let scores = raw
            .iter()
            .map(|s| (self.labels[&s.id].clone(), s.score))
            .collect();
        let checks: Vec<_> = self
            .checks
            .iter()
            .enumerate()
            .map(|(index, check)| CheckResult {
                index,
                passed: check.passes(&scores, &raw),
            })
            .collect();
        Ok(QualityResult {
            manifest: self.manifest()?,
            protocol_valid: true,
            quality_pass: checks.iter().all(|c| c.passed),
            checks,
            scores,
        })
    }
}
#[cfg(test)]
mod tests;
