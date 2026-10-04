//! Owner-scoped local answer preparation; no model or embedding invocation.
use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;
use personal_ai_llm::local_answer::LocalAnswerPreview;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSelection {
    pub document_id: String,
    pub ordinal: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerPreparation {
    pub request_id: String,
    pub question: String,
    pub endpoint: String,
    pub model: String,
    pub sources: Vec<SourceSelection>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceBinding {
    pub selection: SourceSelection,
    pub content_sha256: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct AnswerMaterial {
    #[serde(flatten)]
    pub selection: SourceSelection,
    pub title: String,
    pub source: String,
    pub text: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct PreparedAnswer {
    pub question: String,
    pub materials: Vec<AnswerMaterial>,
    pub request: LocalAnswerPreview,
    pub request_sha256: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct AnswerAuthorization {
    pub request_id: String,
    pub status: String,
    pub digest: String,
    pub created_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub preview: Option<PreparedAnswer>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerApproval {
    pub digest: String,
    pub acknowledge_sharing: bool,
    pub acknowledge_local_compute: bool,
}
/// Internal, ephemeral claim. Consuming it is irreversible and does not prove a model send.
pub struct AnswerClaim {
    pub request_id: String,
    pub digest: String,
    pub preview: PreparedAnswer,
}
pub trait AnswerAuthorizationStore: Send + Sync {
    fn prepare_answer(
        &self,
        owner: &UserId,
        input: &AnswerPreparation,
    ) -> BoxFuture<'_, StorageResult<AnswerAuthorization>>;
    fn get_answer_authorization(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<AnswerAuthorization>>;
    /// Returns at most 20 newest metadata rows, without original question or evidence.
    fn list_answer_authorizations(
        &self,
        owner: &UserId,
    ) -> BoxFuture<'_, StorageResult<Vec<AnswerAuthorization>>>;
    fn approve_answer(
        &self,
        owner: &UserId,
        request: &str,
        approval: &AnswerApproval,
    ) -> BoxFuture<'_, StorageResult<AnswerAuthorization>>;
    fn cancel_answer(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<AnswerAuthorization>>;
    /// Internal only. At most one caller receives a snapshot; never resets on failure.
    fn claim_answer(
        &self,
        owner: &UserId,
        request: &str,
        digest: &str,
    ) -> BoxFuture<'_, StorageResult<Option<AnswerClaim>>>;
}
