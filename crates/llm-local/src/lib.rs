#![forbid(unsafe_code)]
//! Explicit local chat-completions SSE profile. Verified backends are recorded separately.
pub mod answer;

use personal_ai_llm::{
    BoxFuture, ChatRequest, LlmError, LlmResult, Role,
    local::{LocalInference, LocalTarget, MAX_PROMPT_BYTES, OUTPUT_TOKENS},
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

fn wire_payload(target: &LocalTarget, request: &ChatRequest) -> LlmResult<serde_json::Value> {
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
    Ok(json!({
        "model": target.model(), "messages": messages, "stream": true,
        "response_format": {"type":"json_object"}, "max_tokens": request.max_output_tokens.unwrap_or(OUTPUT_TOKENS),
        "temperature": request.temperature.unwrap_or(0.0), "n": 1,
        "chat_template_kwargs": {"enable_thinking": false}
    }))
}

pub struct LocalChatClient {
    client: reqwest::Client,
}
impl LocalChatClient {
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
        let payload = wire_payload(target, request)?;
        let mut response = self
            .client
            .post(format!("{}/v1/chat/completions", target.endpoint()))
            .json(&payload)
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
            .is_none_or(|v| v.split(';').next() != Some("text/event-stream"))
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
impl LocalInference for LocalChatClient {
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
    choices: Vec<Choice>,
    error: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct Choice {
    index: u32,
    delta: Delta,
    finish_reason: Option<String>,
}
#[derive(Deserialize)]
struct Delta {
    role: Option<String>,
    content: Option<String>,
    reasoning_content: Option<String>,
    refusal: Option<String>,
    tool_calls: Option<Vec<serde_json::Value>>,
    function_call: Option<serde_json::Value>,
}
struct Parser<'a> {
    model: &'a str,
    line: Vec<u8>,
    data: Option<Vec<u8>>,
    bytes: usize,
    sequence: u64,
    terminal: bool,
    done: bool,
    text: TextAssembly,
}
impl<'a> Parser<'a> {
    fn new(model: &'a str) -> Self {
        Self {
            model,
            line: Vec::new(),
            data: None,
            bytes: 0,
            sequence: 0,
            terminal: false,
            done: false,
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
                self.line(sink)?;
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
    fn line(&mut self, sink: &dyn TextDeltaSink) -> LlmResult<()> {
        let line = self.line.strip_suffix(b"\r").unwrap_or(&self.line);
        if line.is_empty() {
            if let Some(data) = self.data.take() {
                self.record(&data, sink)?;
            }
            return Ok(());
        }
        if line.starts_with(b":") && !self.done {
            std::str::from_utf8(line).map_err(|_| invalid())?;
            return Ok(());
        }
        if self.data.is_some() || self.done {
            return Err(invalid());
        }
        let data = line.strip_prefix(b"data:").ok_or_else(invalid)?;
        self.data = Some(data.strip_prefix(b" ").unwrap_or(data).to_vec());
        Ok(())
    }
    fn record(&mut self, data: &[u8], sink: &dyn TextDeltaSink) -> LlmResult<()> {
        if data == b"[DONE]" {
            if !self.terminal || self.done {
                return Err(invalid());
            }
            self.done = true;
            return Ok(());
        }
        if self.terminal || self.done {
            return Err(invalid());
        }
        let record: Record = serde_json::from_slice(data).map_err(|_| invalid())?;
        if record.model != self.model || record.error.is_some() || record.choices.len() != 1 {
            return Err(invalid());
        }
        let choice = &record.choices[0];
        let delta = &choice.delta;
        if choice.index != 0
            || delta.role.as_deref().is_some_and(|r| r != "assistant")
            || delta
                .reasoning_content
                .as_ref()
                .is_some_and(|s| !s.is_empty())
            || delta.refusal.as_ref().is_some_and(|s| !s.is_empty())
            || delta.tool_calls.as_ref().is_some_and(|v| !v.is_empty())
            || delta.function_call.is_some()
            || choice.finish_reason.as_deref().is_some_and(|r| r != "stop")
        {
            return Err(invalid());
        }
        let content = delta.content.as_deref().unwrap_or_default();
        if self.text.partial_text().map_or(0, str::len) + content.len() > MAX_OUTPUT {
            return Err(invalid());
        }
        self.text
            .apply(self.sequence, TextEvent::Delta(content))
            .map_err(|_| invalid())?;
        self.sequence += 1;
        if !content.is_empty() {
            sink.delta(content);
        }
        self.terminal = choice.finish_reason.is_some();
        Ok(())
    }
    fn finish(mut self) -> LlmResult<String> {
        if !self.terminal || !self.done || !self.line.is_empty() || self.data.is_some() {
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
