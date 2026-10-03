//! Official `ChatGPT` plan access. Deliberately separate from API-key money budgets.
mod oauth;
mod stream;

pub use oauth::{CodeExchange, PendingLogin, Registration};
use reqwest::{Client, Response, header::HeaderValue};
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const AUTH: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
const MAX_BODY: usize = 1024 * 1024;
pub type Result<T> = std::result::Result<T, Error>;

/// Static diagnostics never include tokens, callback URLs or provider bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error(pub &'static str);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}

pub(crate) fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| Error("invalid system time"))
}

pub(crate) fn bearer(token: &str) -> Result<HeaderValue> {
    if token.is_empty() || token.len() > 32768 {
        return Err(Error("invalid access token"));
    }
    let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| Error("invalid access token"))?;
    value.set_sensitive(true);
    Ok(value)
}

pub(crate) async fn body(mut response: Response) -> Result<Vec<u8>> {
    match response.status().as_u16() {
        200..=299 => (),
        401 => return Err(Error("session rejected; sign in again")),
        403 => return Err(Error("ChatGPT plan access denied")),
        429 => return Err(Error("ChatGPT usage limit reached")),
        _ => return Err(Error("OpenAI request failed; no automatic retry")),
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error("response interrupted"))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_BODY {
            return Err(Error("response too large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[derive(Deserialize)]
pub struct Model {
    pub slug: String,
    pub display_name: String,
    visibility: String,
}

#[derive(Deserialize)]
struct Catalog {
    models: Vec<Model>,
}

/// Fixed official endpoints, no redirects, no provider fallback or inference retries.
pub struct ChatGptClient {
    client: Client,
    auth: String,
    resource: String,
}
impl ChatGptClient {
    /// # Errors
    /// Fails when the TLS client cannot be built.
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(90))
            .build()
            .map_err(|_| Error("HTTP client unavailable"))?;
        Ok(Self {
            client,
            auth: AUTH.into(),
            resource: RESOURCE.into(),
        })
    }

    /// # Errors
    /// Requires an unexpired token with both plan and resource permissions.
    pub async fn models(&self, registration: &Registration) -> Result<Vec<Model>> {
        let token = registration.access_token()?;
        let response = self
            .client
            .get(format!("{}/models", self.resource))
            .header(reqwest::header::AUTHORIZATION, bearer(token)?)
            .send()
            .await
            .map_err(|_| Error("model catalog unavailable"))?;
        let catalog: Catalog = serde_json::from_slice(&body(response).await?)
            .map_err(|_| Error("invalid model catalog"))?;
        Ok(catalog
            .models
            .into_iter()
            .filter(|m| m.visibility == "list")
            .collect())
    }

    /// One explicit request; returns text only after a successful terminal event.
    /// No request is retried on an unknown outcome.
    /// # Errors
    /// Fails for missing consent, invalid input, quota errors or incomplete streams.
    pub async fn ask(
        &self,
        registration: &Registration,
        model: &str,
        prompt: &str,
        consent: bool,
    ) -> Result<String> {
        if !consent
            || model.is_empty()
            || model.len() > 128
            || model.chars().any(char::is_control)
            || prompt.trim().is_empty()
            || prompt.len() > 32768
        {
            return Err(Error(
                "explicit subscription usage consent and bounded input required",
            ));
        }
        self.response(registration, serde_json::json!({"model":model, "input":[{"role":"user", "content":prompt}], "store":false, "stream":true})).await
    }

    /// Send the frozen RSS scoring messages using subscription-compatible instructions.
    /// API-only sampling and token-budget fields are deliberately absent on the wire.
    /// # Errors
    /// Rejects missing consent, unexpected message roles, oversized input and incomplete output.
    pub async fn score_value(
        &self,
        registration: &Registration,
        model: &str,
        request: &personal_ai_llm::ChatRequest,
        consent: bool,
    ) -> Result<String> {
        use personal_ai_llm::Role;
        let [system, user] = request.messages.as_slice() else {
            return Err(Error("scoring requires exactly two frozen messages"));
        };
        if !consent
            || model.is_empty()
            || model.len() > 128
            || model.chars().any(char::is_control)
            || system.role != Role::System
            || user.role != Role::User
            || system.content.trim().is_empty()
            || user.content.trim().is_empty()
            || system.content.len().saturating_add(user.content.len())
                > personal_ai_agent_core::feed_value::MAX_INPUT_BYTES
        {
            return Err(Error(
                "invalid scoring input or missing subscription consent",
            ));
        }
        let text=self.response(registration,serde_json::json!({"model":model,"instructions":system.content,"input":[{"role":"user","content":user.content}],"store":false,"stream":true})).await?;
        if text.len() > personal_ai_agent_core::feed_value::MAX_OUTPUT_BYTES {
            return Err(Error("scoring output too large"));
        }
        Ok(text)
    }

    async fn response(
        &self,
        registration: &Registration,
        payload: serde_json::Value,
    ) -> Result<String> {
        let token = registration.access_token()?;
        let mut response = self
            .client
            .post(format!("{}/responses", self.resource))
            .header(reqwest::header::AUTHORIZATION, bearer(token)?)
            .json(&payload)
            .send()
            .await
            .map_err(|_| Error("inference outcome unknown; not retried"))?;
        if !response.status().is_success() {
            body(response).await?;
            return Err(Error("inference rejected"));
        }
        if response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .is_none_or(|v| {
                v.split(';')
                    .next()
                    .is_none_or(|mime| mime.trim() != "text/event-stream")
            })
        {
            return Err(Error("expected event stream"));
        }
        let mut stream = stream::TextStream::default();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| Error("inference stream interrupted; not retried"))?
        {
            if let Some(text) = stream.push(&chunk)? {
                return Ok(text);
            }
        }
        Err(Error("inference ended without completion; not retried"))
    }
}

#[cfg(test)]
mod tests;
