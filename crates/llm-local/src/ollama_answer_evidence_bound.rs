//! Synthetic-only schema constraint: supported decisions must carry nonempty evidence.
use super::{
    AnswerProvider, AnswerSource, BoxFuture, GuardedAnswers, GuardedDiagnostics, GuardedPreview,
    LlmResult, LocalTarget, ModelAnswer, NativeStageGuard,
};
use serde_json::{Value, json};
use std::sync::Arc;

pub const PROFILE: &str = "ollama-knowledge-answer-v19-evidence-bound";

/// Tightens only the verification schema and its matching declaration in the system message.
/// # Errors
/// Preserves source/question bounds and rejects any unexpected internal request shape.
pub fn preview(
    target: &LocalTarget,
    question: &str,
    sources: &[AnswerSource],
) -> LlmResult<GuardedPreview> {
    use super::super::super::{Residency, native_body};
    let invalid = super::super::super::invalid;
    let mut result = super::question_first::preview(target, question, sources)?;
    let mut schema = result.verification_body["format"].clone();
    let item = &schema["properties"]["checks"]["items"];
    if !item.is_object() || item["properties"]["evidence"]["minItems"] != 0 {
        return Err(invalid());
    }
    // Self-contained alternatives avoid implementation-dependent merging of sibling properties.
    let mut supported = item.clone();
    supported["properties"]["evidence"]["minItems"] = json!(1);
    supported["properties"]["support"]["enum"] = json!(["stated", "unavailable"]);
    let mut unsupported = item.clone();
    unsupported["properties"]["support"]["enum"] = json!(["unsupported"]);
    schema["properties"]["checks"]["items"] = json!({"anyOf":[supported, unsupported]});
    let user = result.verification_body["messages"][1]["content"]
        .as_str()
        .ok_or_else(invalid)?;
    let parsed: Value = serde_json::from_str(user).map_err(|_| invalid())?;
    // Use the original request validator for the enlarged system message and combined budget.
    let prompt = super::super::prepare(question, sources)?;
    let mut body = native_body(
        target,
        &prompt,
        &parsed,
        super::super::contract::VERIFY_INSTRUCTION,
        &schema,
        Residency::Reuse,
    )?;
    if body["messages"][1]["content"]
        .as_str()
        .ok_or_else(invalid)?
        .len()
        != user.len()
    {
        return Err(invalid());
    }
    // Preserve the exact question-first user bytes after validating the equal-length encoding.
    body["messages"][1]["content"] = json!(user);
    result.verification_body = body;
    result.profile = PROFILE;
    Ok(result)
}

pub struct EvidenceBoundAnswers {
    inner: GuardedAnswers,
}
impl EvidenceBoundAnswers {
    /// # Errors
    /// Rejects HTTP initialization failures without loading or downloading a model.
    pub fn new(target: LocalTarget, guard: Arc<dyn NativeStageGuard>) -> LlmResult<Self> {
        let mut inner = GuardedAnswers::new(target, guard)?;
        inner.preview = preview;
        Ok(Self { inner })
    }
    /// # Errors
    /// Requires three independent native permissions and the unchanged strict decoder.
    #[must_use = "Await the future to run three separately guarded static requests."]
    pub fn answer_with_diagnostics(
        &self,
        question: &str,
        sources: &[AnswerSource],
    ) -> BoxFuture<'_, LlmResult<(ModelAnswer, GuardedDiagnostics)>> {
        self.inner.answer_with_diagnostics(question, sources)
    }
}
impl AnswerProvider for EvidenceBoundAnswers {
    fn answer(
        &self,
        question: &str,
        sources: &[AnswerSource],
    ) -> BoxFuture<'_, LlmResult<ModelAnswer>> {
        self.inner.answer(question, sources)
    }
}

#[cfg(test)]
#[path = "ollama_answer_evidence_bound_tests.rs"]
mod tests;
