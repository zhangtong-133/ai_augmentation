//! Native Ollama candidate profile. Only the synthetic benchmark wires this adapter.
use crate::{LocalChatClient, MAX_LINE, MAX_OUTPUT, MAX_WIRE, invalid, unavailable};
use personal_ai_llm::{
    AnswerProvider, AnswerSource, BoxFuture, LlmResult, ModelAnswer,
    answer::prepare,
    local::{CONTEXT_TOKENS, LocalTarget, OUTPUT_TOKENS, wire_payload},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PROFILE: &str = "ollama-knowledge-answer-v5";
pub const RUNNER: &str = "llamacpp";
pub const PROTOCOL_STAGES: &[&str] = &[
    "native_stream",
    "native_completion",
    "review_fields",
    "review_json",
    "review_contract",
    "review_quote",
    "review_source",
    "review_support",
    "review_requirements",
];
fn protocol_error(stage: &'static str) -> personal_ai_llm::LlmError {
    personal_ai_llm::LlmError::InvalidResponse(format!("ollama answer protocol: {stage}"))
}
#[path = "ollama_answer_contract.rs"]
mod contract;
#[derive(Debug, Serialize)]
pub struct OllamaAnswerPreview {
    profile: &'static str,
    endpoint: String,
    protocol_sha256: String,
    body: serde_json::Value,
}
impl OllamaAnswerPreview {
    #[must_use]
    pub fn body(&self) -> &serde_json::Value {
        &self.body
    }
    /// # Errors
    /// Rejects serialization failures. This digest does not grant private execution consent.
    pub fn fingerprint(&self) -> LlmResult<String> {
        let bytes = serde_json::to_vec(self).map_err(|_| invalid())?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}
/// Offline exact request, with the same evidence protocol and byte limits as local v2.
/// # Errors
/// Rejects invalid or oversized messages before network access.
pub fn preview(
    target: &LocalTarget,
    question: &str,
    sources: &[AnswerSource],
) -> LlmResult<OllamaAnswerPreview> {
    let prompt = prepare(question, sources)?;
    let schema = contract::schema(prompt.schema(), sources.len());
    let mut request = prompt.request();
    request.messages[0].content.push_str(contract::INSTRUCTION);
    request.messages[0].content.push_str(&schema.to_string());
    let validated = wire_payload(target, &request)?;
    Ok(OllamaAnswerPreview {
        profile: PROFILE,
        endpoint: format!("{}/api/chat", target.endpoint()),
        protocol_sha256: prompt.fingerprint()?,
        body: serde_json::json!({
            "model": target.model(), "runner": RUNNER, "messages": validated["messages"],
            "format": schema, "stream": true, "think": false, "keep_alive": 0,
            "truncate": false, "shift": false,
            "options": { "num_ctx": CONTEXT_TOKENS, "num_predict": OUTPUT_TOKENS,
                "temperature": 0, "seed": 0, "top_k": 1, "top_p": 1,
                "repeat_penalty": 1, "presence_penalty": 0, "frequency_penalty": 0 }
        }),
    })
}
pub struct OllamaAnswers {
    target: LocalTarget,
    client: LocalChatClient,
}
impl OllamaAnswers {
    /// # Errors
    /// Rejects unavailable HTTP client initialization. No model is pulled or loaded here.
    pub fn new(target: LocalTarget) -> LlmResult<Self> {
        Ok(Self {
            target,
            client: LocalChatClient::new()?,
        })
    }
    async fn send(
        &self,
        preview: OllamaAnswerPreview,
        sources: &[AnswerSource],
    ) -> LlmResult<ModelAnswer> {
        let mut response = self
            .client
            .client
            .post(&preview.endpoint)
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
            .and_then(|v| v.to_str().ok())
            .is_none_or(|v| v.split(';').next() != Some("application/x-ndjson"))
        {
            return Err(invalid());
        }
        let mut parser = Parser::new(self.target.model());
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            parser
                .push(&chunk)
                .map_err(|_| protocol_error("native_stream"))?;
        }
        let completed = parser
            .finish()
            .map_err(|_| protocol_error("native_completion"))?;
        contract::decode(&completed, sources)
    }
}
impl AnswerProvider for OllamaAnswers {
    fn answer(
        &self,
        question: &str,
        sources: &[AnswerSource],
    ) -> BoxFuture<'_, LlmResult<ModelAnswer>> {
        let request = preview(&self.target, question, sources).map(|v| (v, sources.to_vec()));
        Box::pin(async move {
            let (preview, sources) = request?;
            self.send(preview, &sources).await
        })
    }
}

#[derive(Deserialize)]
struct Record {
    model: String,
    message: Message,
    done: bool,
    done_reason: Option<String>,
    error: Option<serde_json::Value>,
    remote_model: Option<String>,
    remote_host: Option<String>,
}
#[derive(Deserialize)]
struct Message {
    role: String,
    content: String,
    thinking: Option<String>,
    tool_calls: Option<Vec<serde_json::Value>>,
}
struct Parser<'a> {
    model: &'a str,
    bytes: usize,
    line: Vec<u8>,
    text: String,
    done: bool,
}
impl<'a> Parser<'a> {
    fn new(model: &'a str) -> Self {
        Self {
            model,
            bytes: 0,
            line: Vec::new(),
            text: String::new(),
            done: false,
        }
    }
    fn push(&mut self, data: &[u8]) -> LlmResult<()> {
        self.bytes = self.bytes.checked_add(data.len()).ok_or_else(invalid)?;
        if self.bytes > MAX_WIRE {
            return Err(invalid());
        }
        for &byte in data {
            if self.done {
                return Err(invalid());
            }
            if byte == b'\n' {
                self.record()?;
                self.line.clear();
            } else {
                if self.line.len() >= MAX_LINE {
                    return Err(invalid());
                }
                self.line.push(byte);
            }
        }
        Ok(())
    }
    fn record(&mut self) -> LlmResult<()> {
        let record: Record = serde_json::from_slice(&self.line).map_err(|_| invalid())?;
        if record.model != self.model
            || record.message.role != "assistant"
            || record.error.is_some()
            || record.remote_model.as_ref().is_some_and(|s| !s.is_empty())
            || record.remote_host.as_ref().is_some_and(|s| !s.is_empty())
            || record
                .message
                .thinking
                .as_ref()
                .is_some_and(|s| !s.is_empty())
            || record
                .message
                .tool_calls
                .as_ref()
                .is_some_and(|v| !v.is_empty())
            || record.done_reason.as_deref() != record.done.then_some("stop")
            || self.text.len() + record.message.content.len() > MAX_OUTPUT
        {
            return Err(invalid());
        }
        self.text.push_str(&record.message.content);
        self.done = record.done;
        Ok(())
    }
    fn finish(self) -> LlmResult<String> {
        if !self.done || !self.line.is_empty() || self.text.trim().is_empty() {
            return Err(invalid());
        }
        Ok(self.text)
    }
}

#[cfg(test)]
#[path = "ollama_answer_tests.rs"]
mod tests;
