//! Single-request Ollama native answers, using the exact consent preview body.
use crate::{invalid, ollama_answer::Parser, unavailable};
use personal_ai_llm::{
    AnswerProvider, AnswerSource, BoxFuture, LlmResult, ModelAnswer,
    answer::decode,
    local::LocalTarget,
    local_answer::{AnswerBackend, preview_for},
};
use std::time::Duration;

pub struct OllamaLocalAnswers {
    target: LocalTarget,
    client: reqwest::Client,
}
impl OllamaLocalAnswers {
    /// # Errors
    /// Rejects unavailable HTTP client initialization.
    pub fn new(target: LocalTarget) -> LlmResult<Self> {
        Ok(Self {
            target,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_mins(1))
                .build()
                .map_err(|_| unavailable())?,
        })
    }
}
impl AnswerProvider for OllamaLocalAnswers {
    fn answer(
        &self,
        question: &str,
        sources: &[AnswerSource],
    ) -> BoxFuture<'_, LlmResult<ModelAnswer>> {
        let preview = preview_for(AnswerBackend::Ollama, &self.target, question, sources);
        Box::pin(async move {
            let preview = preview?;
            let mut response = self
                .client
                .post(preview.endpoint())
                .json(preview.body())
                .send()
                .await
                .map_err(|_| unavailable())?;
            if !response.status().is_success() {
                return Err(unavailable());
            }
            if response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_none_or(|value| value.split(';').next() != Some("application/x-ndjson"))
            {
                return Err(invalid());
            }
            let mut parser = Parser::new(self.target.model());
            while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
                parser.push(&chunk)?;
            }
            decode(&parser.finish()?)
        })
    }
}

#[cfg(test)]
#[path = "ollama_local_answer_tests.rs"]
mod tests;
