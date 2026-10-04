//! Cross-process status observation through the existing authorization store.
//! No model execution, unvalidated text, replay log, or background task.
use super::{
    ApiError, AppState, Empty, HeaderMap, Path, Query, QueryRejection, Response, Router, State,
    StatusCode, auth, error, get, id, invalid, runtime,
};
use axum::response::{
    IntoResponse,
    sse::{Event, Sse},
};
use futures_util::stream;
use personal_ai_domain::UserId;
use serde_json::json;
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{LazyLock, Mutex},
    time::Duration,
};
use tokio::{
    sync::{Semaphore, SemaphorePermit},
    time::{Instant, sleep_until, timeout_at},
};

static CONNECTIONS: Semaphore = Semaphore::const_new(32);
static OWNERS: LazyLock<Mutex<HashMap<String, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
pub(super) struct Lease {
    _permit: SemaphorePermit<'static>,
    owner: String,
}
impl Lease {
    pub(super) fn acquire(owner: &UserId) -> Result<Self, ApiError> {
        let limited = || ApiError(StatusCode::TOO_MANY_REQUESTS, "learning_stream_limit");
        let permit = CONNECTIONS.try_acquire().map_err(|_| limited())?;
        let mut owners = OWNERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = owners.entry(owner.to_string()).or_default();
        if *count >= 2 {
            return Err(limited());
        }
        *count += 1;
        Ok(Self {
            _permit: permit,
            owner: owner.to_string(),
        })
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut owners = OWNERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(count) = owners.get_mut(&self.owner) {
            *count -= 1;
            if *count == 0 {
                owners.remove(&self.owner);
            }
        }
    }
}
pub(super) const LIFETIME: Duration = Duration::from_secs(20);
pub(super) const READ_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/learning/model-authorizations/{request}/events",
        get(events),
    )
}
struct Observation {
    state: AppState,
    headers: HeaderMap,
    owner: UserId,
    request: String,
    last_status: Option<String>,
    sequence: u64,
    deadline: Instant,
    next_poll: Instant,
    _lease: Lease,
}
fn event(request: &str, sequence: u64, name: &str, detail: &serde_json::Value) -> Event {
    Event::default().event(name).id(sequence.to_string()).data(
        json!({
            "protocol_version":"learning-status-v1", "request_id":request,
            "sequence":sequence.to_string(), "detail":detail,
        })
        .to_string(),
    )
}
pub(super) fn terminal(status: &str) -> bool {
    !matches!(status, "draft" | "authorized" | "running")
}
pub(super) async fn status(
    state: &AppState,
    headers: &HeaderMap,
    owner: &UserId,
    request: &str,
) -> Result<String, ApiError> {
    let user = auth::current_user(state, headers).await?;
    if user.id != *owner {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok(runtime(state)?
        .get_model_authorization(owner, request)
        .await
        .map_err(error)?
        .status)
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
    // Reject unauthorized/missing requests before committing SSE response headers.
    timeout_at(
        Instant::now() + READ_TIMEOUT,
        status(&state, &headers, &user.id, &request),
    )
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "learning_unavailable"))??;
    let observation = Observation {
        state,
        headers,
        owner: user.id,
        request,
        last_status: None,
        sequence: 0,
        deadline,
        next_poll: Instant::now(),
        _lease: lease,
    };
    let stream = stream::unfold(Some(observation), |state| async move {
        let mut state = state?;
        sleep_until(state.next_poll.min(state.deadline)).await;
        if Instant::now() >= state.deadline {
            let end = event(
                &state.request,
                state.sequence,
                "closed",
                &json!({"reason":"observation_timeout"}),
            );
            return Some((Ok::<_, Infallible>(end), None));
        }
        let snapshot = timeout_at(
            state.deadline.min(Instant::now() + READ_TIMEOUT),
            status(&state.state, &state.headers, &state.owner, &state.request),
        )
        .await;
        let current = match snapshot {
            Ok(Ok(value)) => value,
            failure => {
                let reason = match failure {
                    Ok(Err(ApiError(StatusCode::UNAUTHORIZED, _))) => "session_unavailable",
                    Ok(Err(ApiError(StatusCode::NOT_FOUND, _))) => "request_unavailable",
                    _ => "observation_unavailable",
                };
                let end = event(
                    &state.request,
                    state.sequence,
                    "closed",
                    &json!({"reason":reason}),
                );
                return Some((Ok(end), None));
            }
        };
        state.next_poll = Instant::now() + Duration::from_secs(1);
        if state.last_status.as_ref() == Some(&current) {
            return Some((Ok(Event::default().comment("keep-alive")), Some(state)));
        }
        let done = terminal(&current);
        let next = event(
            &state.request,
            state.sequence,
            "status",
            &json!({"status":current,"terminal":done}),
        );
        state.sequence += 1;
        state.last_status = Some(current);
        Some((Ok(next), if done { None } else { Some(state) }))
    });
    let mut response = Sse::new(stream).into_response();
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().expect("static header"));
    Ok(response)
}
