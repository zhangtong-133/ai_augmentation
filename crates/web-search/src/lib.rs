#![forbid(unsafe_code)]
//! 固定 `SearXNG` 服务的单次搜索适配器；不访问结果链接。
use personal_ai_tools::{BoxFuture, Tool, ToolContext, ToolError, ToolRequest, ToolResponse};
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashSet, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
    acknowledge_external_request: bool,
}
fn default_limit() -> usize {
    5
}
fn invalid() -> ToolError {
    ToolError::InvalidArguments("invalid web search arguments or consent".into())
}
fn failed() -> ToolError {
    ToolError::ExecutionFailed("web search unavailable".into())
}
fn input(request: &ToolRequest) -> Result<Input, ToolError> {
    let value: Input = serde_json::from_str(&request.arguments_json).map_err(|_| invalid())?;
    if value.query.trim().is_empty()
        || value.query.chars().count() > 500
        || value.query.chars().any(char::is_control)
        || !(1..=5).contains(&value.limit)
        || !value.acknowledge_external_request
    {
        return Err(invalid());
    }
    Ok(value)
}
#[derive(Deserialize)]
struct Response {
    results: Vec<ResultItem>,
    #[serde(default)]
    unresponsive_engines: Vec<serde_json::Value>,
}
#[derive(Deserialize)]
struct ResultItem {
    url: String,
    title: String,
    #[serde(default)]
    content: Option<String>,
}
fn plain(value: &str, limit: usize) -> (String, bool) {
    let fragment = scraper::Html::parse_fragment(value);
    let text = fragment.root_element().text().collect::<Vec<_>>().join(" ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated = text.chars().count() > limit;
    (text.chars().take(limit).collect(), truncated)
}
pub struct WebSearch {
    client: Client,
    endpoint: Url,
}
impl WebSearch {
    /// 固定管理员配置的 /search 端点；允许本地部署，不接受 URL 凭据或查询参数。
    /// # Errors
    /// 配置非法或 HTTP 客户端不可用时返回脱敏错误。
    pub fn new(endpoint: &str) -> Result<Self, ToolError> {
        let endpoint = Url::parse(endpoint).map_err(|_| failed())?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || endpoint.path() != "/search"
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(failed());
        }
        let client = Client::builder()
            .no_proxy()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(3))
            .build()
            .map_err(|_| failed())?;
        Ok(Self { client, endpoint })
    }
    async fn search(&self, input: Input) -> Result<ToolResponse, ToolError> {
        let mut response = self
            .client
            .post(self.endpoint.clone())
            .header("accept", "application/json")
            .form(&[
                ("q", input.query.as_str()),
                ("format", "json"),
                ("categories", "general"),
                ("pageno", "1"),
            ])
            .send()
            .await
            .map_err(|_| failed())?;
        if !response.status().is_success()
            || response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(';').next())
                .is_none_or(|s| !s.trim().eq_ignore_ascii_case("application/json"))
        {
            return Err(failed());
        }
        if response.content_length().is_some_and(|n| n > 262_144) {
            return Err(failed());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| failed())? {
            if bytes.len() + chunk.len() > 262_144 {
                return Err(failed());
            }
            bytes.extend_from_slice(&chunk);
        }
        let response: Response = serde_json::from_slice(&bytes).map_err(|_| failed())?;
        if response.results.len() > 100 {
            return Err(failed());
        }
        let mut seen = HashSet::new();
        let mut results = Vec::new();
        for result in response.results {
            if result.url.len() > 2048 {
                continue;
            }
            let Ok(url) = Url::parse(&result.url) else {
                continue;
            };
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                continue;
            }
            if !seen.insert(url.to_string()) {
                continue;
            }
            let (title, title_truncated) = plain(&result.title, 200);
            let (snippet, snippet_truncated) = plain(result.content.as_deref().unwrap_or(""), 1000);
            results.push(json!({"url":url.as_str(),"title":title,"snippet":snippet,"text_truncated":title_truncated||snippet_truncated}));
        }
        let truncated = results.len() > input.limit;
        results.truncate(input.limit);
        Ok(ToolResponse {content:json!({"provider":"searxng","untrusted":true,"partial":!response.unresponsive_engines.is_empty(),"results_truncated":truncated,"results":results}).to_string(),is_error:false})
    }
}
impl Tool for WebSearch {
    fn name(&self) -> &'static str {
        "web_search"
    }
    fn description(&self) -> &'static str {
        "向管理员配置的搜索服务发送查询，可能产生服务费用；需确认对外发送。返回不可信搜索资料，不打开结果链接。"
    }
    fn input_schema_json(&self) -> &'static str {
        r#"{"type":"object","properties":{"query":{"type":"string","minLength":1,"maxLength":500},"limit":{"type":"integer","minimum":1,"maximum":5,"default":5},"acknowledge_external_request":{"type":"boolean","const":true,"description":"同意将查询发送给配置的搜索服务及其搜索引擎，并承担可能的服务费用"}},"required":["query","acknowledge_external_request"],"additionalProperties":false}"#
    }
    fn validate(&self, request: &ToolRequest) -> Result<(), ToolError> {
        input(request).map(|_| ())
    }
    fn execute(
        &self,
        _context: &ToolContext,
        request: &ToolRequest,
    ) -> BoxFuture<'_, Result<ToolResponse, ToolError>> {
        let input = input(request);
        Box::pin(async move { self.search(input?).await })
    }
}
#[cfg(test)]
mod tests;
