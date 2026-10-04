//! Local-only inference: explicit endpoint/model, without subscription or API credentials.
use crate::{BoxFuture, ChatRequest, LlmError, LlmResult, stream::TextDeltaSink};

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
