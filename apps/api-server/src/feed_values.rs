//! Session-owned subscription scoring consent and results. HTTP never dispatches a model.
use crate::{ApiError, AppState, auth};
use axum::{
    Json, Router,
    extract::{
        Path, Query, Request, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    StorageError, StorageResult,
    feed_value::{
        FeedValueStore, ValueApproval, ValuePricing, ValueQuotePlanner, ValueReview, ValueSnapshot,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
mod local;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/feed-values", get(list).post(preview))
        .route("/api/feed-values/config", get(local::config))
        .route("/api/feed-values/local", post(local::preview))
        .route("/api/feed-values/{id}", get(detail))
        .route("/api/feed-values/{id}/audit", get(audit))
        .route("/api/feed-values/{id}/reading", get(reading))
        .route("/api/feed-values/{id}/approve", post(approve))
        .route("/api/feed-values/{id}/approve-local", post(local::approve))
        .route("/api/feed-values/{id}/cancel", post(cancel))
        .layer(middleware::from_fn(no_store))
}
async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    after: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    id: String,
    connection_id: String,
    connection_revision: String,
    model: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    digest: String,
    acknowledge_sharing: bool,
    acknowledge_subscription_usage: bool,
}
fn invalid() -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, "invalid_feed_value")
}
fn id(value: &str) -> Result<String, ApiError> {
    let id = uuid::Uuid::parse_str(value).map_err(|_| invalid())?;
    if id.is_nil() {
        return Err(invalid());
    }
    Ok(id.to_string())
}
#[allow(clippy::needless_pass_by_value)]
fn error(error: StorageError) -> ApiError {
    match error {
        StorageError::NotFound => ApiError(StatusCode::NOT_FOUND, "feed_value_not_found"),
        StorageError::Conflict(_) => ApiError(StatusCode::CONFLICT, "feed_value_conflict"),
        StorageError::InvalidData(_) => invalid(),
        StorageError::Unavailable(_) => {
            ApiError(StatusCode::SERVICE_UNAVAILABLE, "feed_values_unavailable")
        }
    }
}
fn runtime(state: &AppState) -> Result<&dyn FeedValueStore, ApiError> {
    state.feed_values.as_deref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "feed_values_unavailable",
    ))
}
fn body<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    body.map(|v| v.0)
        .map_err(|e| ApiError(e.status(), "invalid_feed_value"))
}
fn output(saved: &ValueReview) -> Value {
    let pricing = match &saved.pricing {
        ValuePricing::Subscription {
            provider,
            model,
            configuration_version,
            connection_id,
            valid_until_unix_ms,
        } => {
            json!({"kind":"subscription","provider":provider,"model":model,"configuration_version":configuration_version,"connection_id":connection_id,"valid_until_unix_ms":valid_until_unix_ms.to_string()})
        }
        ValuePricing::Api { .. } => json!({"kind":"api","available":false}),
        ValuePricing::Local {
            endpoint,
            model,
            valid_until_unix_ms,
            ..
        } => {
            json!({"kind":"local","endpoint":endpoint,"model":model,"valid_until_unix_ms":valid_until_unix_ms.to_string()})
        }
    };
    json!({"id":saved.request_id,"status":saved.status,"digest":saved.digest,"pricing":pricing,"created_at_unix_ms":saved.created_at_unix_ms.to_string(),"expires_at_unix_ms":saved.expires_at_unix_ms.to_string(),"approved_at_unix_ms":saved.approved_at_unix_ms.map(|v|v.to_string()),"execution_mode":"local_only"})
}
fn full(owner: &UserId, saved: &ValueReview) -> Result<Value, ApiError> {
    let plan = saved
        .snapshot
        .as_ref()
        .map(|s| {
            personal_ai_agent_core::feed_value::plan_value_scoring(
                owner,
                &saved.request_id,
                s.day_start_unix_ms,
                s.as_of_unix_ms,
                &s.keywords,
                &s.candidates,
            )
        })
        .transpose()
        .map_err(|_| error(StorageError::Unavailable("invalid saved plan".into())))?
        .flatten();
    let mut result = output(saved);
    result["shared_content"] = plan.as_ref().map_or(Value::Null, |p| {
        let request = p.request();
        json!({"instructions":request.messages[0].content,"input":request.messages[1].content})
    });
    result["candidates"]=plan.as_ref().map_or(Value::Null,|p|json!(p.brief().items.iter().enumerate().map(|(i,item)|json!({"id":i+1,"title":item.entry.title,"subscription_id":item.entry.subscription_id,"entry_key":item.entry.entry_key})).collect::<Vec<_>>()));
    result["scores"] = json!(saved.scores.as_ref().map(|scores| {
        scores
            .iter()
            .map(|score| json!({"id":score.id,"score":score.score,"reason":score.reason}))
            .collect::<Vec<_>>()
    }));
    Ok(result)
}
struct Planner {
    pricing: ValuePricing,
    available: bool,
}
impl ValueQuotePlanner for Planner {
    fn quote(&self, _: &UserId, _: &str, _: &ValueSnapshot) -> StorageResult<ValuePricing> {
        if !self.available {
            return Err(StorageError::Conflict("connection changed".into()));
        }
        Ok(self.pricing.clone())
    }
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<Page>, QueryRejection>,
) -> Result<Response, ApiError> {
    let owner = auth::current_user(&state, &headers).await?;
    let page = query.map_err(|_| invalid())?.0;
    let after = page.after.as_deref().map(id).transpose()?;
    let page = runtime(&state)?
        .list_feed_values(&owner.id, after.as_deref())
        .await
        .map_err(error)?;
    Ok(Json(json!({"items":page.items.iter().map(output).collect::<Vec<_>>(),"next_cursor":page.next_cursor})).into_response())
}
async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    query: Result<Query<Empty>, QueryRejection>,
) -> Result<Response, ApiError> {
    let owner = auth::current_user(&state, &headers).await?;
    query.map_err(|_| invalid())?;
    let saved = runtime(&state)?
        .get_feed_value(&owner.id, &id(&key)?)
        .await
        .map_err(error)?;
    Ok(Json(full(&owner.id, &saved)?).into_response())
}
async fn reading(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    query: Result<Query<Empty>, QueryRejection>,
) -> Result<Response, ApiError> {
    let owner = auth::current_user(&state, &headers).await?;
    query.map_err(|_| invalid())?;
    let saved = runtime(&state)?
        .get_feed_value(&owner.id, &id(&key)?)
        .await
        .map_err(error)?;
    if saved.status != "succeeded" {
        return Err(ApiError(StatusCode::CONFLICT, "feed_value_not_readable"));
    }
    let corrupt = || ApiError(StatusCode::SERVICE_UNAVAILABLE, "feed_values_unavailable");
    let snapshot = saved.snapshot.as_ref().ok_or_else(corrupt)?;
    let scores = saved.scores.as_ref().ok_or_else(corrupt)?;
    let plan = personal_ai_agent_core::feed_value::plan_value_scoring(
        &owner.id,
        &saved.request_id,
        snapshot.day_start_unix_ms,
        snapshot.as_of_unix_ms,
        &snapshot.keywords,
        &snapshot.candidates,
    )
    .map_err(|_| corrupt())?
    .ok_or_else(corrupt)?;
    let items = personal_ai_agent_core::feed_value::value_reading_items(&plan, scores)
        .map_err(|_| corrupt())?;
    Ok(Json(json!({
        "id":saved.request_id,"digest":saved.digest,"status":"succeeded",
        "day_start_unix_ms":snapshot.day_start_unix_ms.to_string(),
        "as_of_unix_ms":snapshot.as_of_unix_ms.to_string(),
        "keywords":plan.brief().keywords,"items":items,
    }))
    .into_response())
}
async fn audit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    query: Result<Query<Empty>, QueryRejection>,
) -> Result<Response, ApiError> {
    let owner = auth::current_user(&state, &headers).await?;
    query.map_err(|_| invalid())?;
    let events = runtime(&state)?
        .feed_value_audit(&owner.id, &id(&key)?)
        .await
        .map_err(error)?;
    Ok(Json(json!({"items":events.iter().map(|e|json!({"event":e.event,"at_unix_ms":e.at_unix_ms.to_string()})).collect::<Vec<_>>()})).into_response())
}
async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<Empty>, QueryRejection>,
    input: Result<Json<Preview>, JsonRejection>,
) -> Result<Response, ApiError> {
    let owner = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    query.map_err(|_| invalid())?;
    let input = body(input)?;
    let request = id(&input.id)?;
    let connection = id(&input.connection_id)?;
    let revision = input
        .connection_revision
        .parse::<i64>()
        .map_err(|_| invalid())?;
    if !(1..=1000).contains(&revision)
        || revision.to_string() != input.connection_revision
        || input.model.trim().is_empty()
        || input.model.len() > 128
        || input.model.chars().any(char::is_control)
    {
        return Err(invalid());
    }
    let store = runtime(&state)?;
    let connections = state.subscription_connections.as_deref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "subscription_connections_unavailable",
    ))?;
    let c = connections
        .get_subscription_connection(&owner.id, &connection)
        .await
        .map_err(error)?;
    let planner = Planner {
        available: c.status == "active"
            && c.revision == revision
            && c.models.contains(&input.model),
        pricing: ValuePricing::Subscription {
            provider: "chatgpt-plan".into(),
            model: input.model,
            configuration_version: c.configuration_version(),
            connection_id: c.id,
            valid_until_unix_ms: c.valid_until_unix_ms,
        },
    };
    let saved = store
        .preview_feed_value(&owner.id, &request, Arc::new(planner))
        .await
        .map_err(error)?;
    if !matches!(saved.pricing, ValuePricing::Subscription { .. }) {
        return Err(error(StorageError::Conflict(
            "different scoring mode".into(),
        )));
    }
    Ok(Json(full(&owner.id, &saved)?).into_response())
}
async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    query: Result<Query<Empty>, QueryRejection>,
    input: Result<Json<Approval>, JsonRejection>,
) -> Result<Response, ApiError> {
    let owner = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    query.map_err(|_| invalid())?;
    let input = body(input)?;
    if input.digest.len() != 64
        || !input
            .digest
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(invalid());
    }
    let saved = runtime(&state)?
        .approve_feed_value(
            &owner.id,
            &id(&key)?,
            &ValueApproval {
                digest: input.digest,
                currency: None,
                amount: None,
                acknowledge_sharing: input.acknowledge_sharing,
                acknowledge_cost: false,
                acknowledge_subscription_usage: input.acknowledge_subscription_usage,
                acknowledge_local_compute: false,
            },
        )
        .await
        .map_err(error)?;
    Ok(Json(full(&owner.id, &saved)?).into_response())
}
async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    query: Result<Query<Empty>, QueryRejection>,
    input: Result<Json<Empty>, JsonRejection>,
) -> Result<Response, ApiError> {
    let owner = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    query.map_err(|_| invalid())?;
    body(input)?;
    let saved = runtime(&state)?
        .cancel_feed_value(&owner.id, &id(&key)?)
        .await
        .map_err(error)?;
    Ok(Json(full(&owner.id, &saved)?).into_response())
}
