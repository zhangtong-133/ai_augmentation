//! Local-only answer adapter and exact wire preview, without private-data execution wiring.
use crate::LocalChatClient;
use personal_ai_llm::local_answer::request;
pub use personal_ai_llm::local_answer::{LocalAnswerPreview, PROFILE, preview};
use personal_ai_llm::{
    AnswerProvider, AnswerSource, BoxFuture, LlmResult, ModelAnswer,
    answer::prepare,
    local::{LocalInference, LocalTarget},
    stream::IgnoreTextDeltas,
};

pub struct LocalAnswers {
    target: LocalTarget,
    client: LocalChatClient,
}
impl LocalAnswers {
    /// # Errors
    /// Rejects unavailable local HTTP client initialization.
    pub fn new(target: LocalTarget) -> LlmResult<Self> {
        Ok(Self {
            target,
            client: LocalChatClient::new()?,
        })
    }
}
impl AnswerProvider for LocalAnswers {
    fn answer(
        &self,
        question: &str,
        sources: &[AnswerSource],
    ) -> BoxFuture<'_, LlmResult<ModelAnswer>> {
        let prompt = prepare(question, sources);
        Box::pin(async move {
            let output = self
                .client
                .infer(&self.target, &request(&prompt?), &IgnoreTextDeltas)
                .await?;
            personal_ai_llm::answer::decode(&output)
        })
    }
}

#[cfg(test)]
#[path = "answer_tests.rs"]
mod tests;
