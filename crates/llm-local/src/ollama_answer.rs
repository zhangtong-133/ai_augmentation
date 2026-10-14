//! Native Ollama candidate profile. Only the synthetic benchmark wires this adapter.
use crate::{LocalChatClient, MAX_LINE, MAX_OUTPUT, MAX_WIRE, invalid, unavailable};
use personal_ai_llm::{
    AnswerProvider, AnswerSource, BoxFuture, LlmResult, ModelAnswer,
    answer::prepare,
    local::{CONTEXT_TOKENS, LocalTarget, OUTPUT_TOKENS, wire_payload},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PROFILE: &str = "ollama-knowledge-answer-v14";
pub const REUSE_PROFILE: &str = "ollama-knowledge-answer-v15-reuse";
pub const RUNNER: &str = "llamacpp";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Residency {
    Cold,
    Reuse,
}
impl Residency {
    #[must_use]
    pub fn profile(self) -> &'static str {
        match self {
            Self::Cold => PROFILE,
            Self::Reuse => REUSE_PROFILE,
        }
    }
    fn keep_alive(self) -> u32 {
        match self {
            Self::Cold => 0,
            Self::Reuse => 30,
        }
    }
}
pub const PROTOCOL_STAGES: &[&str] = &[
    "native_guard",
    "native_stream",
    "native_completion",
    "selection_fields",
    "selection_json",
    "selection_decision",
    "selection_evidence",
    "selection_quote",
    "selection_bounds",
    "verification_fields",
    "verification_json",
    "verification_decision",
    "verification_evidence",
    "verification_quote",
    "verification_bounds",
];
fn protocol_error(stage: &'static str) -> personal_ai_llm::LlmError {
    personal_ai_llm::LlmError::InvalidResponse(format!("ollama answer protocol: {stage}"))
}
#[path = "ollama_answer_extract.rs"]
mod contract;
#[path = "ollama_answer_diagnostics.rs"]
mod diagnostics;
pub use diagnostics::SemanticDiagnostics;
#[derive(Debug, Serialize)]
pub struct OllamaAnswerPreview {
    profile: &'static str,
    endpoint: String,
    protocol_sha256: String,
    body: serde_json::Value,
    verification_body: serde_json::Value,
}
impl OllamaAnswerPreview {
    #[must_use]
    pub fn body(&self) -> &serde_json::Value {
        &self.body
    }
    #[must_use]
    pub fn verification_body(&self) -> &serde_json::Value {
        &self.verification_body
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
    preview_with_residency(target, question, sources, Residency::Cold)
}
/// Offline exact requests for a finite residency policy; no model is loaded here.
/// # Errors
/// Rejects invalid or oversized messages before network access.
pub fn preview_with_residency(
    target: &LocalTarget,
    question: &str,
    sources: &[AnswerSource],
    residency: Residency,
) -> LlmResult<OllamaAnswerPreview> {
    let prompt = prepare(question, sources)?;
    let catalog = contract::catalog(sources)?;
    // Both inputs are frozen before the first inference. The verifier never sees selection output.
    let mut user: serde_json::Value = serde_json::from_str(prompt.user()).map_err(|_| invalid())?;
    user["exact_excerpts"] = serde_json::to_value(&catalog).map_err(|_| invalid())?;
    let build = |instruction: &str, schema: serde_json::Value| -> LlmResult<serde_json::Value> {
        let mut request = prompt.request();
        request.messages[0].content = format!("{instruction}{schema}");
        request.messages[1].content = user.to_string();
        let validated = wire_payload(target, &request)?;
        Ok(serde_json::json!({
            "model": target.model(), "runner": RUNNER, "messages": validated["messages"],
            "format": schema, "stream": true, "think": false, "keep_alive": residency.keep_alive(),
            "truncate": false, "shift": false,
            "options": { "num_ctx": CONTEXT_TOKENS, "num_predict": OUTPUT_TOKENS,
                "temperature": 0, "seed": 0, "top_k": 1, "top_p": 1,
                "repeat_penalty": 1, "presence_penalty": 0, "frequency_penalty": 0 }
        }))
    };
    let body = build(
        contract::SELECT_INSTRUCTION,
        contract::selection_schema(&catalog),
    )?;
    let verification_body = build(
        contract::VERIFY_INSTRUCTION,
        contract::verification_schema(&catalog),
    )?;
    Ok(OllamaAnswerPreview {
        profile: residency.profile(),
        endpoint: format!("{}/api/chat", target.endpoint()),
        protocol_sha256: prompt.fingerprint()?,
        body,
        verification_body,
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerStage {
    Select,
    Verify,
}
impl AnswerStage {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Verify => "verify",
        }
    }
}
/// The application must confirm identity, residency and resource reserve for each native call.
pub trait NativeStageGuard: Send + Sync {
    fn permit(&self, stage: AnswerStage) -> BoxFuture<'_, LlmResult<()>>;
}
pub struct OllamaAnswers {
    target: LocalTarget,
    client: LocalChatClient,
    guard: std::sync::Arc<dyn NativeStageGuard>,
    residency: Residency,
}
impl OllamaAnswers {
    /// # Errors
    /// Rejects unavailable HTTP client initialization. No model is pulled or loaded here.
    pub fn new(
        target: LocalTarget,
        guard: std::sync::Arc<dyn NativeStageGuard>,
    ) -> LlmResult<Self> {
        Self::with_residency(target, guard, Residency::Cold)
    }
    /// # Errors
    /// Rejects unavailable HTTP client initialization. The host guards every native request.
    pub fn with_residency(
        target: LocalTarget,
        guard: std::sync::Arc<dyn NativeStageGuard>,
        residency: Residency,
    ) -> LlmResult<Self> {
        Ok(Self {
            target,
            client: LocalChatClient::new()?,
            guard,
            residency,
        })
    }
    async fn send(
        &self,
        preview: OllamaAnswerPreview,
        sources: &[AnswerSource],
    ) -> LlmResult<(ModelAnswer, SemanticDiagnostics)> {
        let catalog = contract::catalog(sources)?;
        self.guard.permit(AnswerStage::Select).await?;
        let selected = self.post(&preview.endpoint, preview.body()).await?;
        let selection = contract::decode_selection(&selected, sources, &catalog)?;
        self.guard.permit(AnswerStage::Verify).await?;
        let checks = self
            .post(&preview.endpoint, preview.verification_body())
            .await?;
        contract::decode_verification(&checks, selection, sources, &catalog)
    }
    /// Returns bounded aggregate metadata after both responses pass protocol validation.
    /// No model text, excerpt keys or ordered per-requirement judgments are exposed here.
    /// # Errors
    /// Uses the same frozen requests, per-call guards and failure behavior as `answer`.
    #[must_use = "Await the future to run the guarded diagnostic."]
    pub fn answer_with_diagnostics(
        &self,
        question: &str,
        sources: &[AnswerSource],
    ) -> BoxFuture<'_, LlmResult<(ModelAnswer, SemanticDiagnostics)>> {
        let request = preview_with_residency(&self.target, question, sources, self.residency)
            .map(|v| (v, sources.to_vec()));
        Box::pin(async move {
            let (preview, sources) = request?;
            self.send(preview, &sources).await
        })
    }
    async fn post(&self, endpoint: &str, body: &serde_json::Value) -> LlmResult<String> {
        let mut response = self
            .client
            .client
            .post(endpoint)
            .json(body)
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
        parser
            .finish()
            .map_err(|_| protocol_error("native_completion"))
    }
}
impl AnswerProvider for OllamaAnswers {
    fn answer(
        &self,
        question: &str,
        sources: &[AnswerSource],
    ) -> BoxFuture<'_, LlmResult<ModelAnswer>> {
        let result = self.answer_with_diagnostics(question, sources);
        Box::pin(async move { result.await.map(|(answer, _)| answer) })
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
