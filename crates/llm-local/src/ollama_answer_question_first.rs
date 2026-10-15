//! Synthetic-only controlled change: put the unchanged question before all reference data.
use super::{
    AnswerProvider, AnswerSource, BoxFuture, GuardedAnswers, GuardedDiagnostics, GuardedPreview,
    LlmResult, LocalTarget, ModelAnswer, NativeStageGuard,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

pub const PROFILE: &str = "ollama-knowledge-answer-v18-question-first";

// Struct field order is intentional. A serde_json::Value map would sort the question last.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct QuestionFirst {
    question: String,
    evidence: Value,
    exact_excerpts: Value,
}

/// Reorders only the full user JSON's fields; all values and the planning body stay unchanged.
/// # Errors
/// Rejects invalid inputs, prompt bounds and any unexpected internal request shape.
pub fn preview(
    target: &LocalTarget,
    question: &str,
    sources: &[AnswerSource],
) -> LlmResult<GuardedPreview> {
    let mut result = super::preview(target, question, sources)?;
    result.profile = PROFILE;
    for body in [&mut result.selection_body, &mut result.verification_body] {
        let invalid = super::super::super::invalid;
        let original = body["messages"][1]["content"]
            .as_str()
            .ok_or_else(invalid)?;
        let ordered: QuestionFirst = serde_json::from_str(original).map_err(|_| invalid())?;
        let content = serde_json::to_string(&ordered).map_err(|_| invalid())?;
        // The original validated byte budget still applies; no extra bytes or truncation.
        if content.len() != original.len() {
            return Err(invalid());
        }
        body["messages"][1]["content"] = Value::String(content);
    }
    Ok(result)
}

pub struct QuestionFirstAnswers {
    inner: GuardedAnswers,
}
impl QuestionFirstAnswers {
    /// # Errors
    /// Rejects HTTP initialization failures; never loads or downloads a model here.
    pub fn new(target: LocalTarget, guard: Arc<dyn NativeStageGuard>) -> LlmResult<Self> {
        let mut inner = GuardedAnswers::new(target, guard)?;
        inner.preview = preview;
        Ok(Self { inner })
    }
    /// # Errors
    /// Requires three fresh native host permissions with the unchanged strict decoder.
    #[must_use = "Await the future to run three separately guarded static requests."]
    pub fn answer_with_diagnostics(
        &self,
        question: &str,
        sources: &[AnswerSource],
    ) -> BoxFuture<'_, LlmResult<(ModelAnswer, GuardedDiagnostics)>> {
        self.inner.answer_with_diagnostics(question, sources)
    }
}
impl AnswerProvider for QuestionFirstAnswers {
    fn answer(
        &self,
        question: &str,
        sources: &[AnswerSource],
    ) -> BoxFuture<'_, LlmResult<ModelAnswer>> {
        self.inner.answer(question, sources)
    }
}

#[cfg(test)]
#[path = "ollama_answer_question_first_tests.rs"]
mod tests;
