//! Durable, owner-scoped execution of an already approved local answer request.
use crate::{BoxFuture, StorageResult, answer_authorizations::PreparedAnswer};
use personal_ai_domain::UserId;
use serde::{Deserialize, Serialize};

/// A bounded model result. Authoritative materials are rebuilt separately, never copied here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredAnswerCitation {
    pub id: usize,
    pub quote: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredAnswerResult {
    pub answer: String,
    pub citations: Vec<StoredAnswerCitation>,
    pub insufficient_evidence: bool,
}

impl From<personal_ai_llm::ModelAnswer> for StoredAnswerResult {
    fn from(value: personal_ai_llm::ModelAnswer) -> Self {
        Self {
            answer: value.answer,
            citations: value
                .citations
                .into_iter()
                .map(|citation| StoredAnswerCitation {
                    id: citation.id,
                    quote: citation.quote,
                })
                .collect(),
            insufficient_evidence: value.insufficient_evidence,
        }
    }
}

impl From<StoredAnswerResult> for personal_ai_llm::ModelAnswer {
    fn from(value: StoredAnswerResult) -> Self {
        Self {
            answer: value.answer,
            citations: value
                .citations
                .into_iter()
                .map(|citation| personal_ai_llm::AnswerCitation {
                    id: citation.id,
                    quote: citation.quote,
                })
                .collect(),
            insufficient_evidence: value.insufficient_evidence,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AnswerExecution {
    pub request_id: String,
    pub status: String,
    pub digest: String,
    pub request_sha256: String,
    pub started_at_unix_ms: i64,
    pub deadline_at_unix_ms: i64,
    pub sent_at_unix_ms: Option<i64>,
    pub finished_at_unix_ms: Option<i64>,
    /// Rebuilt from current documents for valid running/succeeded details only.
    pub preview: Option<PreparedAnswer>,
    /// Present only for a validated succeeded detail. Lists never include it.
    pub result: Option<StoredAnswerResult>,
}

/// Internal, non-serializable claim. Only one caller receives this snapshot and token.
pub struct AnswerExecutionClaim {
    pub owner: UserId,
    pub request_id: String,
    pub token: String,
    pub digest: String,
    pub preview: PreparedAnswer,
}

#[derive(Clone, Debug)]
pub enum AnswerExecutionOutcome {
    Succeeded(StoredAnswerResult),
    Failed,
    Unknown,
}

pub trait AnswerExecutionStore: Send + Sync {
    /// Atomically consumes the old authorization and creates one durable execution.
    fn claim_answer_execution(
        &self,
        owner: &UserId,
        request: &str,
        digest: &str,
    ) -> BoxFuture<'_, StorageResult<Option<AnswerExecutionClaim>>>;
    /// Rechecks all bindings and atomically records the sole permission to send.
    fn begin_answer_execution(
        &self,
        claim: &AnswerExecutionClaim,
    ) -> BoxFuture<'_, StorageResult<bool>>;
    /// Rechecks the live claim while an admitted request is in progress; never sends.
    fn check_answer_execution(
        &self,
        claim: &AnswerExecutionClaim,
    ) -> BoxFuture<'_, StorageResult<bool>>;
    /// Stores only a bounded, source-valid result; never restores permission to send.
    fn finish_answer_execution(
        &self,
        claim: &AnswerExecutionClaim,
        outcome: AnswerExecutionOutcome,
    ) -> BoxFuture<'_, StorageResult<AnswerExecution>>;
    fn get_answer_execution(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<AnswerExecution>>;
    /// At most 20 newest metadata rows; preview and result are always absent.
    fn list_answer_executions(
        &self,
        owner: &UserId,
    ) -> BoxFuture<'_, StorageResult<Vec<AnswerExecution>>>;
    /// Cancels running work or removes a succeeded result without restoring authorization.
    fn cancel_answer_execution(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<AnswerExecution>>;
}
