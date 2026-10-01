use reqwest::{
    Client, Url,
    header::{COOKIE, HeaderMap, HeaderValue},
};
use serde_json::{Value, json};
use std::time::Duration;

pub struct Bridge {
    client: Client,
    base: Url,
    enabled: bool,
}
impl Bridge {
    /// # Errors
    /// 仅允许固定 IPv4 回环地址和用户会话；拒绝 URL 凭据、路径和代理。
    pub fn new(base: &str, token: &str, enabled: bool) -> Result<Self, &'static str> {
        let base = Url::parse(base).map_err(|_| "invalid MCP_API_URL")?;
        if base.scheme() != "http"
            || base.host_str() != Some("127.0.0.1")
            || !base.username().is_empty()
            || base.password().is_some()
            || base.path() != "/"
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err("MCP_API_URL must be an HTTP IPv4 loopback origin");
        }
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid MCP_SESSION_TOKEN");
        }
        let mut headers = HeaderMap::new();
        let mut cookie = HeaderValue::from_str(&format!("personal_ai_session_v2={token}"))
            .map_err(|_| "invalid MCP_SESSION_TOKEN")?;
        cookie.set_sensitive(true);
        headers.insert(COOKIE, cookie);
        let client = Client::builder()
            .no_proxy()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .default_headers(headers)
            .timeout(Duration::from_secs(40))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| "MCP HTTP client unavailable")?;
        Ok(Self {
            client,
            base,
            enabled,
        })
    }
    async fn request(
        &self,
        path: &str,
        call: Option<(&str, Value)>,
    ) -> Result<Value, &'static str> {
        let url = self.base.join(path).map_err(|_| "api_unavailable")?;
        let request = if let Some((id, body)) = call {
            self.client
                .post(url)
                .header("X-Requested-With", "personal-ai")
                .header("Idempotency-Key", id)
                .json(&body)
        } else {
            self.client.get(url)
        };
        let mut response = request
            .send()
            .await
            .map_err(|_| "api_unavailable_or_result_unknown")?;
        if !response.status().is_success() {
            return Err(match response.status().as_u16() {
                401 => "session_expired",
                403 => "permission_denied",
                404 => "tool_unavailable",
                409 => "request_already_used_or_conflicting",
                429 => "tool_limit_reached",
                _ => "api_unavailable_or_result_unknown",
            });
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "api_result_unknown")? {
            if bytes.len() + chunk.len() > 131_072 {
                return Err("api_response_too_large");
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| "invalid_api_response")
    }
    pub(crate) async fn available(&self) -> Result<bool, &'static str> {
        if !self.enabled {
            return Ok(false);
        }
        let data = self.request("/api/tools", None).await?;
        let tools = data["tools"].as_array().ok_or("invalid_api_response")?;
        Ok(tools
            .iter()
            .any(|t| t["name"] == "knowledge_search" && t["read_only"] == true))
    }
    pub(crate) async fn search(
        &self,
        id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Value, &'static str> {
        if !self.enabled {
            return Err("embedding_cost_not_enabled");
        }
        let result = self
            .request(
                "/api/tools/knowledge_search",
                Some((id, json!({"query":query,"limit":limit}))),
            )
            .await?;
        if result["tool"] != "knowledge_search" || !result["output"]["hits"].is_array() {
            return Err("invalid_api_response");
        }
        // 只返回工具输出和最小审计 ID，不透传潜在新增的服务端元数据。
        Ok(json!({"hits":result["output"]["hits"],"request_id":id}))
    }
}
