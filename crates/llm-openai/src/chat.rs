use personal_ai_llm::{AnswerProvider, AnswerSource, BoxFuture, LlmError, LlmResult, ModelAnswer};
use reqwest::{
    Client, Url,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

pub struct OpenAiAnswers {
    client: Client,
    endpoint: Url,
    model: String,
}
impl OpenAiAnswers {
    /// 创建服务端聊天适配器；模型必须支持 Chat Completions 的严格结构化输出。
    ///
    /// # Errors
    /// URL、模型或凭据配置无效时失败。
    pub fn new(base: &str, key: &str, model: &str) -> LlmResult<Self> {
        let invalid = || LlmError::InvalidRequest("invalid answer configuration".into());
        let mut endpoint = Url::parse(base).map_err(|_| invalid())?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || key.trim().is_empty()
            || model.trim().is_empty()
        {
            return Err(invalid());
        }
        endpoint.set_path(&format!(
            "{}/chat/completions",
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
        })
    }
}
#[derive(Deserialize)]
struct Completion {
    choices: Vec<Choice>,
}
#[derive(Deserialize)]
struct Choice {
    finish_reason: String,
    message: Message,
}
#[derive(Deserialize)]
struct Message {
    content: Option<String>,
    refusal: Option<String>,
}
fn invalid_response() -> LlmError {
    LlmError::InvalidResponse("invalid answer response".into())
}
fn decode(bytes: &[u8]) -> LlmResult<ModelAnswer> {
    let response: Completion = serde_json::from_slice(bytes).map_err(|_| invalid_response())?;
    let [choice] = response.choices.as_slice() else {
        return Err(invalid_response());
    };
    if choice.finish_reason != "stop" || choice.message.refusal.is_some() {
        return Err(invalid_response());
    }
    personal_ai_llm::answer::decode(
        choice
            .message
            .content
            .as_deref()
            .ok_or_else(invalid_response)?,
    )
}
impl AnswerProvider for OpenAiAnswers {
    fn answer(
        &self,
        question: &str,
        sources: &[AnswerSource],
    ) -> BoxFuture<'_, LlmResult<ModelAnswer>> {
        let (question, sources) = (question.to_owned(), sources.to_vec());
        Box::pin(async move {
            let prompt = personal_ai_llm::answer::prepare(&question, &sources)?;
            let payload = json!({
                "model": self.model, "store": false, "stream": false,
                "max_completion_tokens": personal_ai_llm::answer::OUTPUT_TOKENS,
                "messages": [
                    {"role":"system","content":prompt.system()},
                    {"role":"user","content":prompt.user()}
                ],
                "response_format": {"type":"json_schema","json_schema": {
                    "name":"knowledge_answer","strict":true,"schema":prompt.schema()
                }}
            });
            let mut response = self
                .client
                .post(self.endpoint.clone())
                .json(&payload)
                .send()
                .await
                .map_err(|_| LlmError::ProviderUnavailable("answer request failed".into()))?;
            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(LlmError::RateLimited);
            }
            if !response.status().is_success() {
                return Err(LlmError::ProviderUnavailable(
                    "answer request rejected".into(),
                ));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| invalid_response())? {
                if bytes.len() + chunk.len() > 128 * 1024 {
                    return Err(invalid_response());
                }
                bytes.extend_from_slice(&chunk);
            }
            decode(&bytes)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_ai_llm::AnswerCitation;
    #[test]
    fn rejects_refusals_truncation_missing_content_and_unknown_fields() {
        let valid = json!({"answer":"Supported", "citations":[{"id":1,"quote":"untrusted evidence"}], "insufficient_evidence":false})
            .to_string();
        for (reason, refusal, content) in [
            ("length", None, Some(valid.clone())),
            ("stop", Some("refused"), Some(valid.clone())),
            ("stop", None, None),
            ("tool_calls", None, Some(valid.clone())),
            ("stop", None, Some("{}".into())),
            ("stop", None, Some("not json".into())),
        ] {
            let bytes = serde_json::to_vec(&json!({"choices":[{"finish_reason":reason,"message":{"content":content,"refusal":refusal}}]})).unwrap();
            assert!(decode(&bytes).is_err());
        }
        let bytes = serde_json::to_vec(
            &json!({"choices":[{"finish_reason":"stop","message":{"content":valid}}]}),
        )
        .unwrap();
        assert_eq!(
            decode(&bytes).unwrap().citations,
            vec![AnswerCitation {
                id: 1,
                quote: "untrusted evidence".into()
            }]
        );
    }
    #[test]
    fn rejects_legacy_ids_missing_quotes_and_untrusted_citation_metadata() {
        for citations in [
            json!([1]),
            json!([{"id":1}]),
            json!([{"id":1,"quote":null}]),
            json!([{"id":1,"quote":"evidence","title":"forged"}]),
            json!([{"id":1,"quote":"evidence","quote_start":0}]),
        ] {
            let content =
                json!({"answer":"answer","citations":citations,"insufficient_evidence":false})
                    .to_string();
            let bytes = serde_json::to_vec(
                &json!({"choices":[{"finish_reason":"stop","message":{"content":content}}]}),
            )
            .unwrap();
            assert!(decode(&bytes).is_err());
        }
    }
    #[tokio::test]
    async fn chat_contract_has_bounded_structured_evidence_and_sanitized_errors() {
        use axum::{
            Json, Router,
            http::{HeaderMap, StatusCode},
            routing::post,
        };
        for status in [
            StatusCode::OK,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::UNAUTHORIZED,
            StatusCode::TEMPORARY_REDIRECT,
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}/v1", listener.local_addr().unwrap());
            let app = Router::new().route("/v1/chat/completions", post(move |headers: HeaderMap, Json(input): Json<serde_json::Value>| async move {
                assert_eq!(headers["authorization"], "Bearer test-key");
                assert_eq!(input["model"], "chat-fixture");
                assert_eq!(input["store"], false);
                assert_eq!(input["stream"], false);
                assert_eq!(input["max_completion_tokens"], 2048);
                assert!(input.get("tools").is_none());
                assert_eq!(input["response_format"]["json_schema"]["strict"], true);
                let item = &input["response_format"]["json_schema"]["schema"]["properties"]["citations"]["items"];
                assert_eq!(item["required"], json!(["id", "quote"]));
                assert_eq!(item["additionalProperties"], false);
                assert_eq!(item["properties"]["id"]["enum"], json!([1]));
                let evidence: serde_json::Value = serde_json::from_str(input["messages"][1]["content"].as_str().unwrap()).unwrap();
                assert_eq!(evidence["evidence"][0]["text"], "untrusted evidence");
                let body = if status == StatusCode::OK {
                    json!({"choices":[{"finish_reason":"stop","message":{"content":json!({"answer":"Supported", "citations":[{"id":1,"quote":"untrusted evidence"}], "insufficient_evidence":false}).to_string()}}]}).to_string()
                } else { "secret error".into() };
                (status, body)
            }));
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let mut provider = OpenAiAnswers::new(&base, "test-key", "chat-fixture").unwrap();
            let mut headers = HeaderMap::new();
            headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer test-key"));
            provider.client = Client::builder()
                .no_proxy()
                .default_headers(headers)
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap();
            let result = provider
                .answer(
                    "question",
                    &[AnswerSource {
                        id: 1,
                        text: "untrusted evidence".into(),
                    }],
                )
                .await;
            if status == StatusCode::OK {
                assert_eq!(result.unwrap().answer, "Supported");
            } else {
                let error = result.unwrap_err();
                assert!(!error.to_string().contains("secret"));
                if status == StatusCode::TOO_MANY_REQUESTS {
                    assert_eq!(error, LlmError::RateLimited);
                }
            }
            assert!(provider.answer("question", &[]).await.is_err());
            server.abort();
        }
    }
}
