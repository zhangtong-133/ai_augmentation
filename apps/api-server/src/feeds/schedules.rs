use super::{Empty, Page, error, id, invalid, is_digest, output, page, payload, revision, runtime};
use crate::{ApiError, AppState, auth};
use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
};
use personal_ai_feeds::schedule::ScheduleInput;
use serde::Deserialize;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/feed-subscriptions/{id}/schedules", post(preview))
        .route("/api/feed-schedules", get(list))
        .route("/api/feed-schedules/{id}", get(detail))
        .route("/api/feed-schedules/{id}/audit", get(audit))
        .route("/api/feed-schedules/{id}/approve", post(approve))
        .route("/api/feed-schedules/{id}/cancel", post(cancel))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    schedule_id: String,
    starts_at_unix_ms: String,
    ends_at_unix_ms: String,
    interval_hours: u8,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Consent {
    accepted_digest: String,
    acknowledge_recurring_source_requests: bool,
}
async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Result<Json<Preview>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    let saved = runtime(&state)?
        .schedules
        .preview_feed_schedule(
            &user.id,
            &id(&key)?,
            &ScheduleInput {
                schedule_id: id(&input.schedule_id)?,
                starts_at_unix_ms: revision(&input.starts_at_unix_ms)?,
                ends_at_unix_ms: revision(&input.ends_at_unix_ms)?,
                interval_hours: input.interval_hours,
            },
        )
        .await
        .map_err(error)?;
    let mut response = output(&saved)?;
    *response.status_mut() = StatusCode::CREATED;
    Ok(response)
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<Page>, QueryRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let after = page(query)?.as_deref().map(id).transpose()?;
    output(
        &runtime(&state)?
            .schedules
            .list_feed_schedules(&user.id, after.as_deref())
            .await
            .map_err(error)?,
    )
}
async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    output(
        &runtime(&state)?
            .schedules
            .get_feed_schedule(&user.id, &id(&key)?)
            .await
            .map_err(error)?,
    )
}
async fn audit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    output(
        &runtime(&state)?
            .schedules
            .feed_schedule_audit(&user.id, &id(&key)?)
            .await
            .map_err(error)?,
    )
}
async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Result<Json<Consent>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    if !input.acknowledge_recurring_source_requests || !is_digest(&input.accepted_digest) {
        return Err(invalid());
    }
    output(
        &runtime(&state)?
            .schedules
            .approve_feed_schedule(&user.id, &id(&key)?, &input.accepted_digest)
            .await
            .map_err(error)?,
    )
}
async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Result<Json<Empty>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    payload(body)?;
    output(
        &runtime(&state)?
            .schedules
            .cancel_feed_schedule(&user.id, &id(&key)?)
            .await
            .map_err(error)?,
    )
}
