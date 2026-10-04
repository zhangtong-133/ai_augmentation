//! Local-only answer adapter and exact wire preview, without private-data execution wiring.
use crate::{LocalChatClient, wire_payload};
use personal_ai_llm::{
    AnswerProvider, AnswerSource, BoxFuture, ChatRequest, LlmResult, ModelAnswer,
    answer::{AnswerPrompt, prepare},
    local::{LocalInference, LocalTarget},
    stream::IgnoreTextDeltas,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const PROFILE: &str = "local-knowledge-answer-v2";
#[derive(Clone, Debug, Serialize)]
pub struct LocalAnswerPreview {
    profile: &'static str,
    endpoint: String,
    protocol_sha256: String,
    body: serde_json::Value,
}
impl LocalAnswerPreview {
    #[must_use]
    pub fn body(&self) -> &serde_json::Value {
        &self.body
    }
    /// Fingerprints the target URL, wire body and versioned application protocol.
    /// # Errors
    /// Rejects serialization failures; this digest is not an authorization token.
    pub fn fingerprint(&self) -> LlmResult<String> {
        let bytes = serde_json::to_vec(self).map_err(|_| crate::invalid())?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}
// json_object constrains JSON syntax only; include the full shape in the system message.
fn request(prompt: &AnswerPrompt) -> ChatRequest {
    let mut request = prompt.request();
    request.messages[0]
        .content
        .push_str("\nRequired JSON schema:\n");
    request.messages[0]
        .content
        .push_str(&prompt.schema().to_string());
    request
}
/// Offline preview; applies the same byte limits as the actual local transport.
/// # Errors
/// Rejects invalid/oversized messages before any network access.
pub fn preview(
    target: &LocalTarget,
    question: &str,
    sources: &[AnswerSource],
) -> LlmResult<LocalAnswerPreview> {
    let prompt = prepare(question, sources)?;
    Ok(LocalAnswerPreview {
        profile: PROFILE,
        endpoint: format!("{}/v1/chat/completions", target.endpoint()),
        protocol_sha256: prompt.fingerprint()?,
        body: wire_payload(target, &request(&prompt))?,
    })
}
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
