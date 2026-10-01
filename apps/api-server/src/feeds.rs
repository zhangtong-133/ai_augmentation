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
use personal_ai_agent_core::feeds::{FeedExecutionError, FeedExecutor};
use personal_ai_feeds::transport::FeedTransport;
use personal_ai_storage::{
    StorageError,
    briefs::BriefStore,
    feeds::{FeedStore, SubscriptionInput},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

mod briefs;

pub struct FeedRuntime {
    briefs: Arc<dyn BriefStore>,
    store: Arc<dyn FeedStore>,
    executor: Option<FeedExecutor>,
}
impl FeedRuntime {
    /// 默认只提供管理/预览；public 模式才允许确认后执行公网 GET。
    /// # Errors
    /// 未知 `RSS_COLLECTION_MODE` 导致启动失败，不隐式启用。
    pub fn from_env<S: FeedStore + BriefStore + 'static>(
        store: Arc<S>,
    ) -> Result<Arc<Self>, String> {
        let enabled = mode(std::env::var("RSS_COLLECTION_MODE").ok().as_deref())?;
        let transport = enabled.then(|| {
            Arc::new(personal_ai_feed_http::PublicFeedTransport) as Arc<dyn FeedTransport>
        });
        Ok(Arc::new(Self::new(store, transport)))
    }
    /// 仅应用装配和测试使用，传输实现不由 HTTP 输入选择。
    #[must_use]
    pub fn new<S: FeedStore + BriefStore + 'static>(
        store: Arc<S>,
        transport: Option<Arc<dyn FeedTransport>>,
    ) -> Self {
        let executor = transport.map(|transport| FeedExecutor::new(store.clone(), transport));
        Self {
            briefs: store.clone(),
            store,
            executor,
        }
    }
}
fn mode(value: Option<&str>) -> Result<bool, String> {
    match value.unwrap_or("disabled") {
        "disabled" => Ok(false),
        "public" => Ok(true),
        _ => Err("RSS_COLLECTION_MODE must be disabled or public".into()),
    }
}
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .merge(briefs::routes())
        .route("/api/feeds/config", get(config))
        .route("/api/feed-subscriptions", get(subscriptions).post(create))
        .route(
            "/api/feed-subscriptions/{id}",
            get(subscription).put(update).delete(delete),
        )
        .route("/api/feed-subscriptions/{id}/entries", get(entries))
        .route("/api/feed-subscriptions/{id}/collections", post(preview))
        .route("/api/feed-collections", get(collections))
        .route("/api/feed-collections/{id}", get(collection))
        .route("/api/feed-collections/{id}/audit", get(audit))
        .route("/api/feed-collections/{id}/confirm", post(confirm))
        .route("/api/feed-collections/{id}/cancel", post(cancel))
        .route("/api/feed-collections/{id}/recover", post(recover))
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
struct Create {
    id: String,
    name: String,
    source_url: String,
    enabled: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    revision: String,
    name: String,
    source_url: String,
    enabled: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    revision: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    request_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirm {
    accepted_digest: String,
    acknowledge_source_request: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
fn invalid() -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, "invalid_feed")
}
fn id(value: &str) -> Result<String, ApiError> {
    let id = uuid::Uuid::parse_str(value).map_err(|_| invalid())?;
    if id.is_nil() {
        return Err(invalid());
    }
    Ok(id.to_string())
}
fn revision(value: &str) -> Result<u64, ApiError> {
    let n = value.parse::<i64>().map_err(|_| invalid())?;
    if n <= 0 || n.to_string() != value {
        return Err(invalid());
    }
    u64::try_from(n).map_err(|_| invalid())
}
fn payload<T>(value: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    value
        .map(|Json(v)| v)
        .map_err(|e| ApiError(e.status(), "invalid_feed"))
}
fn page(value: Result<Query<Page>, QueryRejection>) -> Result<Option<String>, ApiError> {
    Ok(value.map_err(|_| invalid())?.0.after)
}
fn runtime(state: &AppState) -> Result<&FeedRuntime, ApiError> {
    state.feeds.as_deref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "feeds_unavailable",
    ))
}
#[allow(clippy::needless_pass_by_value)]
fn error(value: StorageError) -> ApiError {
    match value {
        StorageError::NotFound => ApiError(StatusCode::NOT_FOUND, "feed_not_found"),
        StorageError::InvalidData(_) => invalid(),
        StorageError::Conflict(_) => ApiError(StatusCode::CONFLICT, "feed_conflict"),
        StorageError::Unavailable(_) => {
            ApiError(StatusCode::SERVICE_UNAVAILABLE, "feeds_unavailable")
        }
    }
}
fn output(value: &impl Serialize) -> Result<Response, ApiError> {
    fn exact(value: &mut Value) {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    if (key.ends_with("_unix_ms")
                        || key == "revision"
                        || key == "subscription_revision"
                        || key == "preference_revision")
                        && value.is_number()
                    {
                        *value = Value::String(value.to_string());
                    } else {
                        exact(value);
                    }
                }
            }
            Value::Array(items) => {
                for value in items {
                    exact(value);
                }
            }
            _ => {}
        }
    }
    let mut value = serde_json::to_value(value)
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "feeds_unavailable"))?;
    exact(&mut value);
    Ok(Json(value).into_response())
}
async fn config(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    auth::current_user(&state, &headers).await?;
    let enabled = runtime(&state)?.executor.is_some();
    output(&json!({"execution_enabled":enabled,"mode":if enabled {"public"} else {"disabled"}}))
}
async fn subscriptions(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<Page>, QueryRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let after = page(query)?.as_deref().map(id).transpose()?;
    output(
        &runtime(&state)?
            .store
            .list_subscriptions(&user.id, after.as_deref())
            .await
            .map_err(error)?,
    )
}
async fn subscription(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    output(
        &runtime(&state)?
            .store
            .get_subscription(&user.id, &id(&key)?)
            .await
            .map_err(error)?,
    )
}
async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<Create>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    let saved = runtime(&state)?
        .store
        .create_subscription(
            &user.id,
            &id(&input.id)?,
            &SubscriptionInput {
                name: input.name,
                source_url: input.source_url,
                enabled: input.enabled,
            },
        )
        .await
        .map_err(error)?;
    let mut response = output(&saved)?;
    *response.status_mut() = StatusCode::CREATED;
    Ok(response)
}
async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Result<Json<Update>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    output(
        &runtime(&state)?
            .store
            .update_subscription(
                &user.id,
                &id(&key)?,
                revision(&input.revision)?,
                &SubscriptionInput {
                    name: input.name,
                    source_url: input.source_url,
                    enabled: input.enabled,
                },
            )
            .await
            .map_err(error)?,
    )
}
async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Result<Json<Revision>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    runtime(&state)?
        .store
        .delete_subscription(&user.id, &id(&key)?, revision(&input.revision)?)
        .await
        .map_err(error)?;
    output(&json!({"deleted":true}))
}
async fn entries(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    query: Result<Query<Page>, QueryRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let after = page(query)?;
    if let Some(after) = &after {
        let hash = after
            .strip_prefix("guid:")
            .or_else(|| after.strip_prefix("link:"))
            .ok_or_else(invalid)?;
        if !is_digest(hash) {
            return Err(invalid());
        }
    }
    output(
        &runtime(&state)?
            .store
            .list_feed_entries(&user.id, &id(&key)?, after.as_deref())
            .await
            .map_err(error)?,
    )
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
        .store
        .preview_collection(&user.id, &id(&key)?, &id(&input.request_id)?)
        .await
        .map_err(error)?;
    let mut response = output(&saved)?;
    *response.status_mut() = StatusCode::CREATED;
    Ok(response)
}
async fn collections(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<Page>, QueryRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let after = page(query)?.as_deref().map(id).transpose()?;
    output(
        &runtime(&state)?
            .store
            .list_collections(&user.id, after.as_deref())
            .await
            .map_err(error)?,
    )
}
async fn collection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    output(
        &runtime(&state)?
            .store
            .get_collection(&user.id, &id(&key)?)
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
            .store
            .collection_audit(&user.id, &id(&key)?)
            .await
            .map_err(error)?,
    )
}
fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
async fn confirm(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Result<Json<Confirm>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    if !input.acknowledge_source_request || !is_digest(&input.accepted_digest) {
        return Err(invalid());
    }
    let request = id(&key)?;
    let runtime = runtime(&state)?;
    // 先核对所有权，即使部署关闭也不把别人的请求作为合法确认目标。
    runtime
        .store
        .get_collection(&user.id, &request)
        .await
        .map_err(error)?;
    let executor = runtime.executor.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "feed_execution_disabled",
    ))?;
    let saved = executor
        .execute(&user.id, &request, &input.accepted_digest)
        .await
        .map_err(|e| match e {
            FeedExecutionError::Busy => ApiError(StatusCode::TOO_MANY_REQUESTS, "feed_busy"),
            FeedExecutionError::Storage(e) => error(e),
            FeedExecutionError::OutcomeUnknown => {
                ApiError(StatusCode::SERVICE_UNAVAILABLE, "feed_outcome_unknown")
            }
        })?;
    output(&saved)
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
            .store
            .cancel_collection(&user.id, &id(&key)?)
            .await
            .map_err(error)?,
    )
}
async fn recover(
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
            .store
            .recover_collection(&user.id, &id(&key)?)
            .await
            .map_err(error)?,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collection_mode_is_explicit_and_fails_closed() {
        assert_eq!(mode(None), Ok(false));
        assert_eq!(mode(Some("disabled")), Ok(false));
        assert_eq!(mode(Some("public")), Ok(true));
        for value in ["", "true", "fixture", "PUBLIC", " public"] {
            assert!(mode(Some(value)).is_err());
        }
    }
}
