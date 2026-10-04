//! Only live temporary text; recheck session and durable source before every packet.
use super::model_events::{LIFETIME, Lease, READ_TIMEOUT, status, terminal};
use super::{
    ApiError, AppState, Empty, HeaderMap, Path, Query, QueryRejection, Response, Router, State,
    StatusCode, auth, get, id, invalid,
};
use axum::response::{
    IntoResponse,
    sse::{Event, Sse},
};
use futures_util::stream;
use personal_ai_domain::UserId;
use personal_ai_storage::learning::review_text::{
    ReviewTextBridge, TextKind, TextSubscription, TextWindow,
};
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::time::{Instant, sleep_until, timeout_at};

/// # Errors
/// An explicitly configured invalid URL fails startup without exposing credentials.
pub fn learning_text_from_env() -> Result<Option<Arc<dyn ReviewTextBridge>>, String> {
    match std::env::var("LEARNING_TEXT_REDIS_URL") {
        Ok(url) if url.is_empty() => Ok(None),
        Ok(url) => personal_ai_storage_redis::RedisReviewText::new(&url)
            .map(|bridge| Some(Arc::new(bridge) as Arc<dyn ReviewTextBridge>))
            .map_err(|_| "invalid learning text bridge configuration".into()),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err("invalid learning text bridge configuration".into()),
    }
}
pub(super) fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/learning/model-authorizations/{request}/text-events",
        get(events),
    )
}
struct Observation {
    state: AppState,
    headers: HeaderMap,
    owner: UserId,
    request: String,
    subscription: Option<Box<dyn TextSubscription>>,
    window: TextWindow,
    last_status: Option<String>,
    sequence: u64,
    deadline: Instant,
    next_poll: Instant,
    _lease: Lease,
}
fn event(state: &mut Observation, name: &str, detail: &Value) -> Event {
    let sequence = state.sequence.to_string();
    state.sequence += 1;
    Event::default().event(name).id(&sequence).data(json!({"protocol_version":"learning-text-v1", "request_id":state.request,"sequence":sequence,"detail":detail}).to_string())
}
fn reason(error: &ApiError) -> &'static str {
    match error.0 {
        StatusCode::UNAUTHORIZED => "session_unavailable",
        StatusCode::NOT_FOUND => "request_unavailable",
        _ => "observation_unavailable",
    }
}
async fn check(state: &mut Observation) -> Result<String, ApiError> {
    timeout_at(
        state.deadline.min(Instant::now() + READ_TIMEOUT),
        status(&state.state, &state.headers, &state.owner, &state.request),
    )
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "learning_unavailable"))?
}
async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request): Path<String>,
    query: Result<Query<Empty>, QueryRejection>,
) -> Result<Response, ApiError> {
    query.map_err(|_| invalid())?;
    if headers.contains_key("last-event-id") {
        return Err(invalid());
    }
    let request = id(&request)?;
    let deadline = Instant::now() + LIFETIME;
    let user = timeout_at(
        Instant::now() + READ_TIMEOUT,
        auth::current_user(&state, &headers),
    )
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "learning_unavailable"))??;
    let lease = Lease::acquire(&user.id)?;
    let initial = timeout_at(
        Instant::now() + READ_TIMEOUT,
        status(&state, &headers, &user.id, &request),
    )
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "learning_unavailable"))??;
    let bridge = state.learning_text.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "learning_text_disabled",
    ))?;
    let subscription = if terminal(&initial) {
        None
    } else {
        Some(
            timeout_at(
                deadline.min(Instant::now() + READ_TIMEOUT),
                bridge.subscribe(&user.id, &request),
            )
            .await
            .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "learning_text_unavailable"))?
            .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "learning_text_unavailable"))?,
        )
    };
    let observation = Observation {
        state,
        headers,
        owner: user.id,
        request,
        subscription,
        window: TextWindow::default(),
        last_status: None,
        sequence: 0,
        deadline,
        next_poll: Instant::now(),
        _lease: lease,
    };
    let stream = stream::unfold(Some(observation), observe_next);
    let mut response = Sse::new(stream).into_response();
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().expect("static header"));
    Ok(response)
}

type Step = (Result<Event, Infallible>, Option<Observation>);
fn closed(mut state: Observation, reason: &str) -> Step {
    (
        Ok(event(&mut state, "closed", &json!({"reason":reason}))),
        None,
    )
}
async fn observe_next(state: Option<Observation>) -> Option<Step> {
    let mut state = state?;
    if Instant::now() >= state.deadline {
        return Some(closed(state, "observation_timeout"));
    }
    // Periodic checks also run when the publisher is silent; each text gets a fresh check.
    let packet = if state.last_status.is_none() {
        None
    } else if let Some(subscription) = state.subscription.as_mut() {
        tokio::select! {
            biased;
            () = sleep_until(state.next_poll.min(state.deadline)) => None,
            packet = subscription.next() => Some(packet),
        }
    } else {
        sleep_until(state.next_poll.min(state.deadline)).await;
        None
    };
    if Instant::now() >= state.deadline {
        return Some(closed(state, "observation_timeout"));
    }
    let current = match check(&mut state).await {
        Ok(current) => current,
        Err(error) => return Some(closed(state, reason(&error))),
    };
    if terminal(&current) {
        return Some((
            Ok(event(
                &mut state,
                "status",
                &json!({"status":current,"terminal":true}),
            )),
            None,
        ));
    }
    if let Some(packet) = packet {
        let packet = match packet {
            Ok(Some(packet)) if state.window.accept(&packet).is_ok() => packet,
            Ok(None) => {
                state.subscription = None;
                return Some((Ok(event(&mut state, "clear", &json!({}))), Some(state)));
            }
            _ => return Some(closed(state, "observation_unavailable")),
        };
        let next = match packet.detail {
            TextKind::Delta { text } if current == "running" => {
                event(&mut state, "delta", &json!({"text":text}))
            }
            TextKind::Delta { .. } => return Some(closed(state, "observation_unavailable")),
            TextKind::Clear => event(&mut state, "clear", &json!({})),
            TextKind::End => {
                state.subscription = None;
                event(&mut state, "clear", &json!({}))
            }
        };
        return Some((Ok(next), Some(state)));
    }
    state.next_poll = Instant::now() + Duration::from_secs(1);
    if state.last_status.as_ref() == Some(&current) {
        return Some((Ok(Event::default().comment("keep-alive")), Some(state)));
    }
    let next = event(
        &mut state,
        "status",
        &json!({"status":current,"terminal":false}),
    );
    state.last_status = Some(current);
    Some((Ok(next), Some(state)))
}
