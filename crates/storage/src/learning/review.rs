//! 用户逐项核验，不是模型评分或客观能力认证。
use serde::{Deserialize, Serialize};
pub const RUBRIC_VERSION: &str = "user-evidence-review-v1";
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Missing,
    Unverified,
    Supported,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewDimension {
    pub verdict: ReviewVerdict,
    pub reason: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewBody {
    pub explanation: ReviewDimension,
    pub work: ReviewDimension,
    pub verification: ReviewDimension,
    pub limitations: ReviewDimension,
}
impl ReviewBody {
    #[must_use]
    pub fn supported(&self) -> bool {
        [
            &self.explanation,
            &self.work,
            &self.verification,
            &self.limitations,
        ]
        .iter()
        .all(|d| d.verdict == ReviewVerdict::Supported)
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Pending,
    Confirmed,
    Invalidated,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct TrainingReview {
    pub request_id: String,
    pub evidence_request_id: String,
    pub rubric_version: String,
    pub body: Option<ReviewBody>,
    pub status: ReviewStatus,
    pub created_at_unix_ms: u64,
    pub confirmation_request_id: Option<String>,
    pub assessment_id: Option<String>,
    pub expected_revision: Option<u64>,
    pub confirmed_score: Option<u8>,
}
#[derive(Clone)]
pub struct ReviewInput {
    pub request_id: String,
    pub evidence_request_id: String,
    pub body: ReviewBody,
}
#[derive(Clone)]
pub struct ReviewConfirmation {
    pub request_id: String,
    pub review_request_id: String,
    pub expected_revision: u64,
    pub score: u8,
}
