#![forbid(unsafe_code)]
//! Ollama native NDJSON adapter. No credentials, redirects, proxy, or inference retry.
use personal_ai_llm::{
    BoxFuture, ChatRequest, LlmError, LlmResult, Role,
    local::{CONTEXT_TOKENS, LocalInference, LocalTarget, MAX_PROMPT_BYTES, OUTPUT_TOKENS},
    stream::{TextAssembly, TextDeltaSink, TextEvent},
};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
const MAX_WIRE: usize = 256 * 1024;
const MAX_LINE: usize = 64 * 1024;
const MAX_OUTPUT: usize = 24 * 1024;
fn invalid() -> LlmError {
    LlmError::InvalidResponse("incomplete or invalid local stream".into())
}
fn unavailable() -> LlmError {
    LlmError::ProviderUnavailable("local inference unavailable".into())
}

pub struct Ollama {
    client: reqwest::Client,
}
impl Ollama {
    /// # Errors
    /// Rejects unavailable HTTP client initialization.
    pub fn new() -> LlmResult<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(60))
                .build()
                .map_err(|_| unavailable())?,
        })
    }
    async fn send(
        &self,
        target: &LocalTarget,
        request: &ChatRequest,
        sink: &dyn TextDeltaSink,
    ) -> LlmResult<String> {
        let prompt_bytes: usize = request.messages.iter().map(|m| m.content.len()).sum();
        if request.messages.is_empty()
            || prompt_bytes > MAX_PROMPT_BYTES
            || request
                .temperature
                .is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v))
            || request
                .max_output_tokens
                .is_some_and(|n| n == 0 || n > OUTPUT_TOKENS)
            || request.messages.iter().any(|m| m.role == Role::Tool)
        {
            return Err(LlmError::InvalidRequest(
                "local prompt exceeds configured budget".into(),
            ));
        }
        let messages: Vec<_> = request.messages.iter().map(|m| json!({"role": match m.role {
            Role::System => "system", Role::User => "user", Role::Assistant => "assistant", Role::Tool => "tool"
        }, "content": m.content})).collect();
        let mut response = self.client.post(format!("{}/api/chat", target.endpoint())).json(&json!({
            "model": target.model(), "messages": messages, "stream": true, "think": false,
            "format": "json", "keep_alive": 0,
            "options": {"num_ctx": CONTEXT_TOKENS, "num_predict": request.max_output_tokens.unwrap_or(OUTPUT_TOKENS),
                "temperature": request.temperature.unwrap_or(0.0)}
        })).send().await.map_err(|_| unavailable())?;
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
        let mut parser = Parser::new(target.model());
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            parser.push(&chunk, sink)?;
        }
        parser.finish()
    }
}
impl LocalInference for Ollama {
    fn infer<'a>(
        &'a self,
        target: &'a LocalTarget,
        request: &'a ChatRequest,
        sink: &'a dyn TextDeltaSink,
    ) -> BoxFuture<'a, LlmResult<String>> {
        Box::pin(self.send(target, request, sink))
    }
}
#[derive(Deserialize)]
struct Record {
    model: String,
    done: bool,
    message: Message,
    done_reason: Option<String>,
    error: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct Message {
    role: String,
    content: String,
    thinking: Option<String>,
    tool_calls: Option<Vec<serde_json::Value>>,
    images: Option<Vec<serde_json::Value>>,
}
struct Parser<'a> {
    model: &'a str,
    pending: Vec<u8>,
    bytes: usize,
    sequence: u64,
    terminal: bool,
    text: TextAssembly,
}
impl<'a> Parser<'a> {
    fn new(model: &'a str) -> Self {
        Self {
            model,
            pending: Vec::new(),
            bytes: 0,
            sequence: 0,
            terminal: false,
            text: TextAssembly::default(),
        }
    }
    fn push(&mut self, data: &[u8], sink: &dyn TextDeltaSink) -> LlmResult<()> {
        self.bytes = self.bytes.checked_add(data.len()).ok_or_else(invalid)?;
        if self.bytes > MAX_WIRE {
            return Err(invalid());
        }
        for &byte in data {
            if byte == b'\n' {
                self.record(sink)?;
                self.pending.clear();
            } else {
                if self.pending.len() >= MAX_LINE {
                    return Err(invalid());
                }
                self.pending.push(byte);
            }
        }
        Ok(())
    }
    fn record(&mut self, sink: &dyn TextDeltaSink) -> LlmResult<()> {
        if self.terminal {
            return Err(invalid());
        }
        let record: Record = serde_json::from_slice(&self.pending).map_err(|_| invalid())?;
        if record.model != self.model
            || record.message.role != "assistant"
            || record.error.is_some()
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
            || record
                .message
                .images
                .as_ref()
                .is_some_and(|v| !v.is_empty())
            || (record.done && record.done_reason.as_deref() != Some("stop"))
            || (!record.done && record.done_reason.is_some())
        {
            return Err(invalid());
        }
        if self.text.partial_text().map_or(0, str::len) + record.message.content.len() > MAX_OUTPUT
        {
            return Err(invalid());
        }
        self.text
            .apply(self.sequence, TextEvent::Delta(&record.message.content))
            .map_err(|_| invalid())?;
        self.sequence += 1;
        if !record.message.content.is_empty() {
            sink.delta(&record.message.content);
        }
        self.terminal = record.done;
        Ok(())
    }
    fn finish(mut self) -> LlmResult<String> {
        if !self.terminal || !self.pending.is_empty() {
            return Err(invalid());
        }
        self.text
            .apply(self.sequence, TextEvent::Completed)
            .map_err(|_| invalid())?;
        self.text
            .take_completed()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(invalid)
    }
}
#[cfg(test)]
mod tests;
