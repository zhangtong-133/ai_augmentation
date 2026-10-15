//! Synthetic-only composition of unchanged v16 planning and v15 independent evidence requests.
use super::{
    AnswerProvider, AnswerSource, AnswerStage, BoxFuture, LlmResult, LocalTarget, ModelAnswer,
    NativeStageGuard, OllamaAnswers, Plan, QuestionDiagnostics, Residency, contract, decode_plan,
    planning_body, prepare,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;

#[path = "ollama_answer_question_first.rs"]
pub mod question_first;

pub const PROFILE: &str = "ollama-knowledge-answer-v17-guarded";
#[derive(Debug, Serialize)]
pub struct GuardedPreview {
    profile: &'static str,
    endpoint: String,
    protocol_sha256: String,
    planning_body: Value,
    selection_body: Value,
    verification_body: Value,
}
impl GuardedPreview {
    /// # Errors
    /// Rejects serialization failures; this digest does not authorize private execution.
    pub fn fingerprint(&self) -> LlmResult<String> {
        use sha2::{Digest, Sha256};
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).map_err(|_| super::super::invalid())?)
        ))
    }
}
/// All three requests are frozen before inference and share no intermediate model output.
/// # Errors
/// Rejects invalid sources/questions and oversized native messages before network access.
pub fn preview(
    target: &LocalTarget,
    question: &str,
    sources: &[AnswerSource],
) -> LlmResult<GuardedPreview> {
    let prompt = prepare(question, sources)?;
    let evidence =
        super::super::preview_with_residency(target, question, sources, Residency::Reuse)?;
    Ok(GuardedPreview {
        profile: PROFILE,
        endpoint: evidence.endpoint,
        protocol_sha256: evidence.protocol_sha256,
        planning_body: planning_body(target, &prompt, question)?,
        selection_body: evidence.body,
        verification_body: evidence.verification_body,
    })
}
#[derive(Debug, Serialize)]
pub struct GuardedDiagnostics {
    schema: &'static str,
    selected_excerpts: usize,
    evidence_agrees: bool,
    verifier_kind_disagreements: Option<usize>,
    question: QuestionDiagnostics,
}
fn decode(
    text: &str,
    plan: Plan,
    selected: &contract::Selected,
    sources: &[AnswerSource],
) -> LlmResult<(ModelAnswer, GuardedDiagnostics)> {
    // Parse the original verifier contract strictly, even though its kind is not authoritative.
    let verification: contract::Verification = contract::parse(text, "verification")?;
    let disagreements = (plan.kinds.len() == verification.checks.len()).then(|| {
        plan.kinds
            .iter()
            .zip(&verification.checks)
            .filter(|(kind, check)| **kind != check.kind)
            .count()
    });
    let keys: HashSet<_> = verification
        .checks
        .iter()
        .flat_map(|check| check.evidence.iter().cloned())
        .collect();
    let agrees = selected.keys == keys;
    let checks: Vec<_> = verification
        .checks
        .into_iter()
        .map(|check| json!({"evidence":check.evidence,"support":check.support}))
        .collect();
    // This validates every key and applies only the source-blind planner's kinds.
    let (mut answer, question) =
        super::decode(&json!({"checks":checks}).to_string(), plan, sources)?;
    if !agrees {
        answer = ModelAnswer {
            answer: String::new(),
            citations: vec![],
            insufficient_evidence: true,
        };
    }
    Ok((
        answer,
        GuardedDiagnostics {
            schema: "ollama-answer-guarded-diagnostic-v1",
            selected_excerpts: selected.keys.len(),
            evidence_agrees: agrees,
            verifier_kind_disagreements: disagreements,
            question,
        },
    ))
}
pub struct GuardedAnswers {
    inner: OllamaAnswers,
    preview: PreviewBuilder,
}
type PreviewBuilder = fn(&LocalTarget, &str, &[AnswerSource]) -> LlmResult<GuardedPreview>;
impl GuardedAnswers {
    /// # Errors
    /// Rejects HTTP initialization failures without loading or downloading a model.
    pub fn new(target: LocalTarget, guard: Arc<dyn NativeStageGuard>) -> LlmResult<Self> {
        Ok(Self {
            inner: OllamaAnswers::with_residency(target, guard, Residency::Reuse)?,
            preview,
        })
    }
    /// # Errors
    /// Requires fresh native host permission for each of three static requests.
    #[must_use = "Await the future to run the three guarded native requests."]
    pub fn answer_with_diagnostics(
        &self,
        question: &str,
        sources: &[AnswerSource],
    ) -> BoxFuture<'_, LlmResult<(ModelAnswer, GuardedDiagnostics)>> {
        let request =
            (self.preview)(&self.inner.target, question, sources).map(|p| (p, sources.to_vec()));
        Box::pin(async move {
            let (preview, sources) = request?;
            self.inner.guard.permit(AnswerStage::Plan).await?;
            let plan = decode_plan(
                &self
                    .inner
                    .post(&preview.endpoint, &preview.planning_body)
                    .await?,
            )?;
            self.inner.guard.permit(AnswerStage::Select).await?;
            let selection = self
                .inner
                .post(&preview.endpoint, &preview.selection_body)
                .await?;
            let catalog = contract::catalog(&sources)?;
            let selected = contract::decode_selection(&selection, &sources, &catalog)?;
            self.inner.guard.permit(AnswerStage::Verify).await?;
            let checked = self
                .inner
                .post(&preview.endpoint, &preview.verification_body)
                .await?;
            decode(&checked, plan, &selected, &sources)
        })
    }
}
impl AnswerProvider for GuardedAnswers {
    fn answer(
        &self,
        question: &str,
        sources: &[AnswerSource],
    ) -> BoxFuture<'_, LlmResult<ModelAnswer>> {
        let result = self.answer_with_diagnostics(question, sources);
        Box::pin(async move { result.await.map(|(answer, _)| answer) })
    }
}
#[cfg(test)]
#[path = "ollama_answer_guarded_tests.rs"]
mod tests;
