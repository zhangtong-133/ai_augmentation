//! `OpenAI` 兼容 Embedding HTTP 适配器。
use personal_ai_llm::{BoxFuture, Embedding, EmbeddingProvider, LlmError, LlmResult};
use reqwest::{
    Client, Url,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

pub struct OpenAiEmbeddings {
    client: Client,
    endpoint: Url,
    model: String,
    dimensions: usize,
}
impl OpenAiEmbeddings {
    /// 配置服务端端点，不接受 URL 内嵌凭据，禁止携带密钥跟随重定向。
    ///
    /// # Errors
    /// 配置不完整或无效时返回错误。
    pub fn new(base: &str, key: &str, model: &str, dimensions: usize) -> LlmResult<Self> {
        let invalid = || LlmError::InvalidRequest("invalid embedding configuration".into());
        let mut endpoint = Url::parse(base).map_err(|_| invalid())?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || key.trim().is_empty()
            || model.trim().is_empty()
            || !(1..=4096).contains(&dimensions)
        {
            return Err(invalid());
        }
        endpoint.set_path(&format!(
            "{}/embeddings",
            endpoint.path().trim_end_matches('/')
        ));
        let mut headers = HeaderMap::new();
        let mut token = HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| invalid())?;
        token.set_sensitive(true);
        headers.insert(AUTHORIZATION, token);
        let client = Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| invalid())?;
        Ok(Self {
            client,
            endpoint,
            model: model.into(),
            dimensions,
        })
    }
}
#[derive(Deserialize)]
struct Response {
    data: Vec<Item>,
    model: String,
}
#[derive(Deserialize)]
struct Item {
    index: usize,
    embedding: Vec<f32>,
}
fn invalid_response() -> LlmError {
    LlmError::InvalidResponse("invalid embedding response".into())
}
fn decode(bytes: &[u8], count: usize, model: &str, dimensions: usize) -> LlmResult<Vec<Embedding>> {
    let mut response: Response = serde_json::from_slice(bytes).map_err(|_| invalid_response())?;
    if response.data.len() != count || response.model != model {
        return Err(invalid_response());
    }
    response.data.sort_by_key(|item| item.index);
    response
        .data
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            if item.index != index
                || item.embedding.len() != dimensions
                || item.embedding.iter().any(|v| !v.is_finite())
                || !item.embedding.iter().any(|v| *v != 0.0)
            {
                return Err(invalid_response());
            }
            Ok(Embedding {
                values: item.embedding,
                model: model.into(),
            })
        })
        .collect()
}
impl EmbeddingProvider for OpenAiEmbeddings {
    fn embedding(&self, input: &[String]) -> BoxFuture<'_, LlmResult<Vec<Embedding>>> {
        let input = input.to_vec();
        Box::pin(async move {
            // 当前分块上限为 1000 字符；每批限制 16 段，避免超大请求与响应。
            if input.is_empty()
                || input.len() > 16
                || input
                    .iter()
                    .any(|s| s.trim().is_empty() || s.chars().count() > 1000)
            {
                return Err(LlmError::InvalidRequest("invalid embedding batch".into()));
            }
            let mut response = self.client.post(self.endpoint.clone()).json(&json!({
                "input": input, "model": self.model, "dimensions": self.dimensions, "encoding_format": "float"
            })).send().await.map_err(|_| LlmError::ProviderUnavailable("embedding request failed".into()))?;
            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(LlmError::RateLimited);
            }
            if !response.status().is_success() {
                return Err(LlmError::ProviderUnavailable(
                    "embedding request rejected".into(),
                ));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| invalid_response())? {
                if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                    return Err(invalid_response());
                }
                bytes.extend_from_slice(&chunk);
            }
            decode(&bytes, input.len(), &self.model, self.dimensions)
        })
    }
}
#[cfg(test)]
mod tests;
