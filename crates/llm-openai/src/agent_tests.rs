use super::*;
use personal_ai_storage::model_agents::AgentBudgetLimits;
use std::sync::{Arc, Mutex};
fn budgets() -> (ModelCallBudget, ModelExecutionConfiguration) {
    let planning = ModelCallBudget {
        configuration_version: "planning-v1".into(),
        provider: "openai".into(),
        model: REPLY_MODEL.into(),
        price_version: "fixture-only".into(),
        counter_version: AGENT_CHAT_COUNTER.into(),
        currency: "USD".into(),
        input_price_per_million: 1,
        output_price_per_million: 1,
        input_token_bound: AGENT_CHAT_INPUT_BOUND,
        output_token_bound: 2048,
        valid_until_unix_ms: i64::MAX,
    };
    let mut answer = planning.clone();
    answer.configuration_version = "answer-v1".into();
    answer.output_token_bound = 1024;
    let mut embedding = planning.clone();
    embedding.configuration_version = "embedding-v1".into();
    embedding.model = AGENT_EMBEDDING_MODEL.into();
    embedding.counter_version = AGENT_EMBEDDING_COUNTER.into();
    embedding.input_token_bound = AGENT_EMBEDDING_INPUT_BOUND;
    embedding.output_token_bound = 0;
    embedding.output_price_per_million = 0;
    (
        planning,
        ModelExecutionConfiguration {
            version: answer.configuration_version.clone(),
            embedding,
            answer,
            limits: AgentBudgetLimits {
                phase_amount: 1000,
                daily_amount: 10000,
                daily_model_calls: 10,
                daily_tool_calls: 10,
            },
        },
    )
}
fn request() -> ChatRequest {
    ChatRequest {
        messages: vec![
            personal_ai_llm::ChatMessage {
                role: Role::System,
                content: "Return JSON".into(),
            },
            personal_ai_llm::ChatMessage {
                role: Role::User,
                content: "研究资料 <|endoftext|>".into(),
            },
        ],
        temperature: None,
        max_output_tokens: Some(2048),
    }
}
fn completion() -> Value {
    json!({"model":REPLY_MODEL,"service_tier":"default","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"{\"searches\":[{\"query\":\"research\",\"limit\":5}]}"}}],"usage":{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}})
}
fn embedding() -> Value {
    json!({"model":AGENT_EMBEDDING_MODEL,"data":[{"index":0,"embedding":vec![0.5;AGENT_EMBEDDING_DIMENSIONS]}],"usage":{"prompt_tokens":10,"total_tokens":10}})
}
#[tokio::test]
async fn agent_policy_requires_frozen_budgets_and_bounds_before_network() {
    let (p, c) = budgets();
    let provider = OpenAiAgentModels::new("fixture", p.clone(), c.clone()).unwrap();
    let body = provider
        .chat_payload(ModelChatStage::Planning, &request(), &p)
        .unwrap();
    assert_eq!(body["response_format"]["json_schema"]["strict"], true);
    assert!(body.get("tools").is_none());
    let mut bad = p.clone();
    bad.input_token_bound -= 1;
    assert!(
        provider
            .chat_payload(ModelChatStage::Planning, &request(), &bad)
            .is_err()
    );
    assert!(
        provider
            .chat_payload(ModelChatStage::Answer, &request(), &p)
            .is_err()
    );
    let mut input = request();
    input.messages[1].role = Role::Assistant;
    assert!(
        provider
            .chat_payload(ModelChatStage::Planning, &input, &p)
            .is_err()
    );
    input = request();
    input.messages[1].content = "x".repeat(49_201);
    assert!(
        provider
            .chat_payload(ModelChatStage::Planning, &input, &p)
            .is_err()
    );
    assert!(
        provider
            .embed(&"字".repeat(1001), &c.embedding)
            .await
            .is_err()
    );
    let mut bad = c.clone();
    bad.embedding.input_token_bound = 100;
    assert!(OpenAiAgentModels::new("fixture", p, bad).is_err());
}
#[test]
fn agent_embedding_usage_requires_complete_consistent_fields_and_checks_contract() {
    let (_, c) = budgets();
    assert_eq!(
        decode_embedding(&serde_json::to_vec(&embedding()).unwrap(), &c.embedding)
            .unwrap()
            .usage
            .unwrap()
            .input_tokens,
        10
    );
    for usage in [
        Value::Null,
        json!({"prompt_tokens":10}),
        json!({"prompt_tokens":10,"total_tokens":11}),
        json!({"prompt_tokens":10,"total_tokens":10,"extra":1}),
        json!({"prompt_tokens":0,"total_tokens":0}),
    ] {
        let mut data = embedding();
        data["usage"] = usage;
        assert!(
            decode_embedding(&serde_json::to_vec(&data).unwrap(), &c.embedding)
                .unwrap()
                .usage
                .is_none()
        );
    }
    for field in ["prompt_tokens", "total_tokens"] {
        let mut data = embedding();
        data["usage"][field] = json!(8193);
        assert!(matches!(
            decode_embedding(&serde_json::to_vec(&data).unwrap(), &c.embedding),
            Err(ReplySendError::ContractViolation)
        ));
    }
    let mut data = embedding();
    data["data"][0]["embedding"] = json!([0.5]);
    assert!(matches!(
        decode_embedding(&serde_json::to_vec(&data).unwrap(), &c.embedding),
        Err(ReplySendError::InvalidResponse)
    ));
}
#[tokio::test]
async fn agent_http_sends_each_stage_once_with_fixed_models_and_usage() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    let app = axum::Router::new().route(
        "/{*path}",
        axum::routing::post(
            move |uri: axum::http::Uri, axum::Json(body): axum::Json<Value>| {
                let captured = captured.clone();
                async move {
                    captured.lock().unwrap().push(body);
                    axum::Json(if uri.path().ends_with("embeddings") {
                        embedding()
                    } else {
                        completion()
                    })
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = Url::parse(&format!("http://{}/v1/", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (p, c) = budgets();
    let provider = OpenAiAgentModels::build(
        "fixture",
        p.clone(),
        c.clone(),
        base,
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(
        provider
            .chat(ModelChatStage::Planning, &request(), &p)
            .await
            .unwrap()
            .usage
            .unwrap()
            .output_tokens,
        20
    );
    assert_eq!(
        provider
            .embed("research", &c.embedding)
            .await
            .unwrap()
            .usage
            .unwrap()
            .input_tokens,
        10
    );
    let mut answer = request();
    answer.max_output_tokens = Some(1024);
    provider
        .chat(ModelChatStage::Answer, &answer, &c.answer)
        .await
        .unwrap();
    let captured = seen.lock().unwrap();
    assert_eq!(captured.len(), 3);
    assert_eq!(captured[0]["max_completion_tokens"], 2048);
    assert_eq!(captured[1]["dimensions"], 1536);
    assert_eq!(captured[2]["max_completion_tokens"], 1024);
    assert_eq!(captured[2]["service_tier"], "default");
    assert_eq!(captured[2]["store"], false);
    server.abort();
}
#[tokio::test]
async fn agent_http_does_not_retry_and_latches_contract_failure() {
    for status in [429, 500, 307, 200] {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = calls.clone();
        let app = axum::Router::new().route(
            "/{*path}",
            axum::routing::post(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                async move {
                    let mut result = completion();
                    result["model"] = json!("wrong-model");
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        axum::Json(result),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/v1/", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (p, c) = budgets();
        let provider = OpenAiAgentModels::build(
            "fixture",
            p.clone(),
            c.clone(),
            base,
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(
            provider
                .chat(ModelChatStage::Planning, &request(), &p)
                .await
                .unwrap_err(),
            if status == 200 {
                ReplySendError::ContractViolation
            } else {
                ReplySendError::Unknown
            }
        );
        if status == 200 {
            assert!(matches!(
                provider.embed("research", &c.embedding).await,
                Err(ReplySendError::InvalidConfiguration)
            ));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }
}

#[tokio::test]
async fn agent_http_timeout_truncated_and_oversized_responses_are_not_retried() {
    for mode in 0..3 {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        let app = axum::Router::new().route(
            "/{*path}",
            axum::routing::post(move || {
                count.fetch_add(1, Ordering::SeqCst);
                async move {
                    if mode == 0 {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    if mode == 2 {
                        "x".repeat(128 * 1024 + 1)
                    } else {
                        "{broken".into()
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/v1/", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (p, c) = budgets();
        let provider =
            OpenAiAgentModels::build("fixture", p.clone(), c, base, Duration::from_millis(100))
                .unwrap();
        assert_eq!(
            provider
                .chat(ModelChatStage::Planning, &request(), &p)
                .await
                .unwrap_err(),
            if mode == 0 {
                ReplySendError::Unknown
            } else {
                ReplySendError::InvalidResponse
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
