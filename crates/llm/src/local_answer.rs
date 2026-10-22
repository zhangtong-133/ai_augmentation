//! Pure local answer request construction, shared by consent and transport.
use crate::{
    AnswerSource, ChatRequest, LlmResult,
    answer::{AnswerPrompt, prepare},
    local::{LocalTarget, wire_payload},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub const PROFILE: &str = "local-knowledge-answer-v2";
pub const OLLAMA_PROFILE: &str = "ollama-local-answer-v1";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnswerBackend {
    #[default]
    Local,
    Ollama,
}
impl AnswerBackend {
    #[must_use]
    pub const fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }
    #[must_use]
    pub const fn profile(self) -> &'static str {
        match self {
            Self::Local => PROFILE,
            Self::Ollama => OLLAMA_PROFILE,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct LocalAnswerPreview {
    profile: &'static str,
    endpoint: String,
    protocol_sha256: String,
    body: serde_json::Value,
}
impl LocalAnswerPreview {
    #[must_use]
    pub fn backend(&self) -> AnswerBackend {
        if self.profile == OLLAMA_PROFILE {
            AnswerBackend::Ollama
        } else {
            AnswerBackend::Local
        }
    }
    #[must_use]
    pub const fn profile(&self) -> &'static str {
        self.profile
    }
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    #[must_use]
    pub fn body(&self) -> &serde_json::Value {
        &self.body
    }
    /// Fingerprints the target URL, wire body and versioned application protocol.
    /// # Errors
    /// Rejects serialization failures; this digest is not an authorization token.
    pub fn fingerprint(&self) -> LlmResult<String> {
        let bytes = serde_json::to_vec(self)
            .map_err(|_| crate::LlmError::InvalidResponse("invalid local answer preview".into()))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}
// json_object constrains JSON syntax only; include the full shape in the system message.
#[must_use]
pub fn request(prompt: &AnswerPrompt) -> ChatRequest {
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

/// Offline preview for a fixed answer backend, including its exact wire body.
/// # Errors
/// Rejects invalid/oversized messages before any network access.
pub fn preview_for(
    backend: AnswerBackend,
    target: &LocalTarget,
    question: &str,
    sources: &[AnswerSource],
) -> LlmResult<LocalAnswerPreview> {
    if backend.is_local() {
        return preview(target, question, sources);
    }
    let prompt = prepare(question, sources)?;
    let payload = wire_payload(target, &request(&prompt))?;
    Ok(LocalAnswerPreview {
        profile: backend.profile(),
        endpoint: format!("{}/api/chat", target.endpoint()),
        protocol_sha256: prompt.fingerprint()?,
        body: serde_json::json!({
            "model": target.model(),
            "runner": "llamacpp",
            "messages": payload["messages"],
            "format": prompt.schema(),
            "stream": true,
            "think": false,
            "keep_alive": 30,
            "truncate": false,
            "shift": false,
            "options": {
                "num_ctx": crate::local::CONTEXT_TOKENS,
                "num_predict": crate::local::OUTPUT_TOKENS,
                "temperature": 0,
                "seed": 0,
                "top_k": 1,
                "top_p": 1,
                "repeat_penalty": 1,
                "presence_penalty": 0,
                "frequency_penalty": 0,
            },
        }),
    })
}

#[cfg(test)]
#[path = "local_answer_tests.rs"]
mod tests;
