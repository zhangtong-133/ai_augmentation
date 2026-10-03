//! Owner-scoped management only; trusted local OAuth binding remains separate.
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
use personal_ai_storage::{
    StorageError,
    subscription_connections::{SubscriptionConnection, SubscriptionConnectionStore},
};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/subscription-connections", get(list))
        .route("/api/subscription-connections/{id}", get(detail))
        .route("/api/subscription-connections/{id}/revoke", post(revoke))
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
struct Revoke {
    revision: String,
}
fn invalid() -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, "invalid_subscription_connection")
}
fn id(value: &str) -> Result<String, ApiError> {
    let value = uuid::Uuid::parse_str(value).map_err(|_| invalid())?;
    if value.is_nil() {
        return Err(invalid());
    }
    Ok(value.to_string())
}
fn runtime(state: &AppState) -> Result<&dyn SubscriptionConnectionStore, ApiError> {
    state.subscription_connections.as_deref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "subscription_connections_unavailable",
    ))
}
#[allow(clippy::needless_pass_by_value)]
fn error(error: StorageError) -> ApiError {
    match error {
        StorageError::NotFound => {
            ApiError(StatusCode::NOT_FOUND, "subscription_connection_not_found")
        }
        StorageError::Conflict(_) => {
            ApiError(StatusCode::CONFLICT, "subscription_connection_conflict")
        }
        StorageError::InvalidData(_) => invalid(),
        StorageError::Unavailable(_) => ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "subscription_connections_unavailable",
        ),
    }
}
// Explicit public fields prevent future runtime identity/credential additions from leaking.
fn output(connection: &SubscriptionConnection) -> Value {
    json!({"id":connection.id,"label":connection.label,"revision":connection.revision.to_string(),"status":connection.status,"models":connection.models,"valid_until_unix_ms":connection.valid_until_unix_ms.to_string()})
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<Page>, QueryRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let page = query.map_err(|_| invalid())?.0;
    let after = page.after.as_deref().map(id).transpose()?;
    let page = runtime(&state)?
        .list_subscription_connections(&user.id, after.as_deref())
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
    let user = auth::current_user(&state, &headers).await?;
    query.map_err(|_| invalid())?;
    let saved = runtime(&state)?
        .get_subscription_connection(&user.id, &id(&key)?)
        .await
        .map_err(error)?;
    Ok(Json(output(&saved)).into_response())
}
async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    query: Result<Query<Empty>, QueryRejection>,
    body: Result<Json<Revoke>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    query.map_err(|_| invalid())?;
    let input = body
        .map_err(|e| ApiError(e.status(), "invalid_subscription_connection"))?
        .0;
    let revision = input.revision.parse::<i64>().map_err(|_| invalid())?;
    if !(1..1000).contains(&revision) || revision.to_string() != input.revision {
        return Err(invalid());
    }
    let saved = runtime(&state)?
        .revoke_subscription_connection(&user.id, &id(&key)?, revision)
        .await
        .map_err(error)?;
    Ok(Json(output(&saved)).into_response())
}
