use super::*;
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use std::sync::{Mutex, atomic::AtomicUsize};

fn context() -> ReplyContext {
    ReplyContext {
        system: "hello".into(),
        user_messages: vec![" world".into()],
        first_sequence: 1,
        max_output_tokens: 1024,
        configuration: ReplyConfiguration {
            model: REPLY_MODEL.into(),
            revision: "config-v1".into(),
        },
    }
}
fn prices() -> ReplyPrices {
    ReplyPrices {
        version: "fixture-price".into(),
        input_per_million: 1_000_000,
        output_per_million: 2_000_000,
        request_limit: 200_000,
        daily_limit: 400_000,
    }
}
fn policy() -> Arc<OpenAiReplyPolicy> {
    Arc::new(OpenAiReplyPolicy::new(context().configuration, prices()).unwrap())
}
fn response() -> Value {
    json!({"model":REPLY_MODEL,"service_tier":"default","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"回答"}}],"usage":{"prompt_tokens":12,"completion_tokens":2,"total_tokens":14,"prompt_tokens_details":{"cached_tokens":5,"audio_tokens":0},"completion_tokens_details":{"reasoning_tokens":0,"audio_tokens":0,"accepted_prediction_tokens":0,"rejected_prediction_tokens":0}}})
}

#[test]
fn fixed_model_counts_plain_text_and_reserves_full_window() {
    let policy = policy();
    assert_eq!(
        policy.count(&context()).unwrap(),
        ReplyTokenCount {
            text_tokens: 2,
            input_token_bound: 128_000
        }
    );
    let mut unicode = context();
    unicode.user_messages = vec!["你好🌍 <|endoftext|>".into()];
    let counted = policy.count(&unicode).unwrap();
    assert!(counted.text_tokens > 2);
    assert!(counted.text_tokens < 128_000);
    let budget = policy.plan(&unicode).unwrap();
    assert_eq!(budget.input_token_bound, 128_000);
    assert_eq!(budget.output_token_bound, 1024);
    assert_eq!(budget.currency, "USD");
    assert_eq!(budget.counter_version, REPLY_COUNTER_VERSION);
    assert_eq!(budget.price_version, "fixture-price");
    policy.disable();
    assert!(policy.plan(&unicode).is_err());
}

#[test]
fn rejects_aliases_invalid_prices_and_context_limits() {
    for model in ["gpt-4o-mini", "gpt-4o", "unknown"] {
        let mut config = context().configuration;
        config.model = model.into();
        assert!(OpenAiReplyPolicy::new(config, prices()).is_err());
    }
    for case in 0..5 {
        let mut prices = prices();
        match case {
            0 => prices.input_per_million = 0,
            1 => prices.output_per_million = 0,
            2 => prices.request_limit = 1,
            3 => prices.daily_limit = 0,
            _ => prices.version = String::new(),
        }
        assert!(OpenAiReplyPolicy::new(context().configuration, prices).is_err());
    }
    let policy = policy();
    for case in 0..10 {
        let mut input = context();
        match case {
            0 => input.configuration.revision = "other".into(),
            1 => input.user_messages.clear(),
            2 => input.user_messages = vec!["x".into(); 17],
            3 => input.user_messages[0] = "x".repeat(4097),
            4 => input.system = "x".repeat(16384),
            5 => input.max_output_tokens = 1025,
            6 => input.system = "\0".into(),
            7 => input.first_sequence = 101,
            8 => input.user_messages[0] = " ".into(),
            _ => input.first_sequence = 0,
        }
        assert!(policy.count(&input).is_err(), "case {case}");
    }
    let mut boundary = context();
    boundary.system = "x".repeat(16384 - 4096 * 3);
    boundary.user_messages = vec!["x".repeat(4096); 3];
    assert!(policy.count(&boundary).is_ok());
    boundary.system.push('x');
    assert!(policy.count(&boundary).is_err());
}

#[test]
fn accepts_only_plain_complete_output_and_complete_usage() {
    let budget = policy().plan(&context()).unwrap();
    let decode_value = |v: &Value| decode(&serde_json::to_vec(v).unwrap(), &budget);
    let valid = decode_value(&response()).unwrap();
    assert_eq!(valid.content, "回答");
    assert_eq!(
        valid.usage,
        Some(ReplyUsage {
            input_tokens: 12,
            output_tokens: 2
        })
    );
    for usage in [
        Value::Null,
        json!({}),
        json!({"prompt_tokens":-1,"completion_tokens":2,"total_tokens":1}),
        json!({"prompt_tokens":12,"completion_tokens":2,"total_tokens":15}),
        json!({"prompt_tokens":12,"completion_tokens":2,"total_tokens":14,"prompt_tokens_details":{"cached_tokens":13}}),
    ] {
        let mut v = response();
        v["usage"] = usage;
        assert_eq!(decode_value(&v).unwrap().usage, None);
    }
    for case in 0..12 {
        let mut v = response();
        match case {
            0 => v["choices"][0]["finish_reason"] = json!("length"),
            1 => v["choices"][0]["message"]["refusal"] = json!("no"),
            2 => v["choices"][0]["message"]["tool_calls"] = json!([]),
            3 => v["choices"][0]["message"]["content"] = Value::Null,
            4 => v["choices"][0]["message"]["content"] = json!(" "),
            5 => v["choices"][0]["message"]["content"] = json!("x".repeat(16385)),
            6 => v["choices"] = json!([]),
            7 => v["choices"][0]["message"]["role"] = json!("user"),
            8 => v["choices"][0]["message"]["function_call"] = json!({}),
            9 => v["choices"][0]["message"]["audio"] = json!({}),
            10 => v["choices"][0]["index"] = json!(1),
            _ => v["choices"][0]["message"]["content"] = json!("\0"),
        }
        assert_eq!(
            decode_value(&v),
            Err(ReplySendError::InvalidResponse),
            "case {case}"
        );
    }
    for case in 0..5 {
        let mut v = response();
        match case {
            0 => v["model"] = json!("gpt-4o-mini"),
            1 => v["service_tier"] = json!("priority"),
            2 => v["usage"]["prompt_tokens"] = json!(128_001),
            3 => v["usage"]["completion_tokens"] = json!(1025),
            _ => v["usage"]["completion_tokens_details"]["reasoning_tokens"] = json!(1),
        }
        assert_eq!(decode_value(&v), Err(ReplySendError::ContractViolation));
    }
}

#[derive(Clone)]
struct ServerState {
    status: StatusCode,
    body: String,
    delay: Duration,
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
}
struct Server {
    endpoint: Url,
    state: ServerState,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn handler(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> (StatusCode, [(&'static str, &'static str); 1], String) {
    assert_eq!(headers[AUTHORIZATION], "Bearer fixture-secret");
    state.calls.fetch_add(1, Ordering::SeqCst);
    state.requests.lock().unwrap().push(input);
    tokio::time::sleep(state.delay).await;
    (state.status, [("location", "/redirected")], state.body)
}
async fn server(status: StatusCode, body: String, delay: Duration) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Url::parse(&format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let state = ServerState {
        status,
        body,
        delay,
        calls: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(vec![])),
    };
    let app = Router::new()
        .route("/v1/chat/completions", post(handler))
        .route("/redirected", post(handler))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server {
        endpoint,
        state,
        task,
    }
}
fn sender(server: &Server, policy: Arc<OpenAiReplyPolicy>, timeout: Duration) -> OpenAiReplies {
    OpenAiReplies::build("fixture-secret", policy, server.endpoint.clone(), timeout).unwrap()
}

#[tokio::test]
async fn sends_frozen_roles_limits_and_no_tools_with_one_http_request() {
    let server = server(StatusCode::OK, response().to_string(), Duration::ZERO).await;
    let policy = policy();
    let context = context();
    let budget = policy.plan(&context).unwrap();
    let sender = sender(&server, policy, Duration::from_secs(2));
    assert!(sender.send_once(&context, &budget).await.is_ok());
    let inputs = server.state.requests.lock().unwrap();
    let body = &inputs[0];
    assert_eq!(
        *body,
        json!({"model":REPLY_MODEL,"messages":[{"role":"system","content":"hello"},{"role":"user","content":" world"}],"max_completion_tokens":1024,"n":1,"stream":false,"store":false,"service_tier":"default"})
    );
    assert_eq!(server.state.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn never_retries_status_errors_redirects_timeout_or_broken_responses() {
    let policy = policy();
    let context = context();
    let budget = policy.plan(&context).unwrap();
    for status in [
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::INTERNAL_SERVER_ERROR,
        StatusCode::UNAUTHORIZED,
        StatusCode::TEMPORARY_REDIRECT,
    ] {
        let server = server(
            status,
            "fixture-secret upstream detail".into(),
            Duration::ZERO,
        )
        .await;
        let result = sender(&server, policy.clone(), Duration::from_secs(2))
            .send_once(&context, &budget)
            .await;
        assert_eq!(result, Err(ReplySendError::Unknown));
        assert_eq!(server.state.calls.load(Ordering::SeqCst), 1);
    }
    for body in ["invalid JSON".into(), "x".repeat(MAX_RESPONSE_BYTES + 1)] {
        let server = server(StatusCode::OK, body, Duration::ZERO).await;
        assert_eq!(
            sender(&server, policy.clone(), Duration::from_secs(2))
                .send_once(&context, &budget)
                .await,
            Err(ReplySendError::InvalidResponse)
        );
        assert_eq!(server.state.calls.load(Ordering::SeqCst), 1);
    }
    let server = server(
        StatusCode::OK,
        response().to_string(),
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(
        sender(&server, policy, Duration::from_millis(100))
            .send_once(&context, &budget)
            .await,
        Err(ReplySendError::Unknown)
    );
    assert_eq!(server.state.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejects_changed_budget_before_network_and_latches_contract_failure() {
    let mut bad = response();
    bad["usage"]["prompt_tokens"] = json!(128_001);
    let server = server(StatusCode::OK, bad.to_string(), Duration::ZERO).await;
    let policy = policy();
    let context = context();
    let budget = policy.plan(&context).unwrap();
    let sender = sender(&server, policy.clone(), Duration::from_secs(2));
    let mut changed = budget.clone();
    changed.price_version = "other".into();
    assert_eq!(
        sender.send_once(&context, &changed).await,
        Err(ReplySendError::InvalidConfiguration)
    );
    assert_eq!(server.state.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        sender.send_once(&context, &budget).await,
        Err(ReplySendError::ContractViolation)
    );
    assert!(policy.plan(&context).is_err());
    assert_eq!(
        sender.send_once(&context, &budget).await,
        Err(ReplySendError::InvalidConfiguration)
    );
    assert_eq!(server.state.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn disconnected_response_is_unknown_and_never_resent() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Url::parse(&format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 8192];
            assert!(stream.read(&mut bytes).await.unwrap() > 0);
            observed.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    let policy = policy();
    let budget = policy.plan(&context()).unwrap();
    let sender =
        OpenAiReplies::build("fixture-secret", policy, endpoint, Duration::from_secs(2)).unwrap();
    assert_eq!(
        sender.send_once(&context(), &budget).await,
        Err(ReplySendError::Unknown)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
}
