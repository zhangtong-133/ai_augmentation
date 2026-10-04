//! Local-only inference: explicit endpoint/model, without subscription or API credentials.
use crate::{BoxFuture, ChatRequest, LlmError, LlmResult, Role, stream::TextDeltaSink};
use serde_json::json;

pub const CONTEXT_TOKENS: u32 = 8192;
pub const OUTPUT_TOKENS: u32 = 2048;
pub const MAX_PROMPT_BYTES: usize = 5632;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalTarget {
    endpoint: String,
    model: String,
}
impl LocalTarget {
    /// # Errors
    /// Only canonical HTTP loopback origins and explicit local model tags are accepted.
    pub fn new(endpoint: &str, model: &str) -> LlmResult<Self> {
        let invalid = || LlmError::InvalidRequest("invalid local endpoint or model".into());
        let address = endpoint.strip_prefix("http://").ok_or_else(invalid)?;
        let socket: std::net::SocketAddr = address.parse().map_err(|_| invalid())?;
        if !socket.ip().is_loopback()
            || socket.port() == 0
            || socket.to_string() != address
            || model.is_empty()
            || model.len() > 128
            || !model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:/-".contains(&b))
            || model.ends_with(":cloud")
            || model.ends_with("-cloud")
        {
            return Err(invalid());
        }
        Ok(Self {
            endpoint: endpoint.into(),
            model: model.into(),
        })
    }
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }
}
pub trait LocalInference: Send + Sync {
    fn infer<'a>(
        &'a self,
        target: &'a LocalTarget,
        request: &'a ChatRequest,
        sink: &'a dyn TextDeltaSink,
    ) -> BoxFuture<'a, LlmResult<String>>;
}

/// Build the exact local transport body without I/O.
/// # Errors
/// Rejects invalid messages or inference parameters and oversized prompts.
pub fn wire_payload(target: &LocalTarget, request: &ChatRequest) -> LlmResult<serde_json::Value> {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn target_rejects_remote_credentials_paths_and_cloud() {
        for endpoint in [
            "https://127.0.0.1:11435",
            "http://localhost:11435",
            "http://192.168.1.1:11435",
            "http://user@127.0.0.1:11435",
            "http://127.0.0.1:11435/",
            "http://127.0.0.1:0",
        ] {
            assert!(LocalTarget::new(endpoint, "qwen3:4b").is_err());
        }
        assert!(LocalTarget::new("http://127.0.0.1:11435", "qwen3:4b-cloud").is_err());
        assert!(LocalTarget::new("http://127.0.0.1:11435", "qwen3:4b-q4_K_M").is_ok());
        assert!(LocalTarget::new("http://[::1]:11435", "qwen3:4b").is_ok());
    }
}
