use crate::{ApiError, AppState, auth};
use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, put},
};
use personal_ai_storage::long_memory::validate;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    title: String,
    content: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    title: String,
    content: String,
    version: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Version {
    version: i64,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default)]
    offset: i64,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/memories", get(list).post(create))
        .route("/api/memories/{id}", put(update).delete(remove))
}
fn invalid() -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, "invalid_memory")
}
fn id(value: &str) -> Result<String, ApiError> {
    Uuid::parse_str(value)
        .map(|id| id.to_string())
        .map_err(|_| invalid())
}
fn body<T>(payload: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    payload
        .map(|Json(value)| value)
        .map_err(|error| ApiError(error.status(), "invalid_memory"))
}

async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    page: Result<Query<Page>, QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let Query(page) = page.map_err(|_| invalid())?;
    if !(0..=100).contains(&page.offset) {
        return Err(invalid());
    }
    let facts = state.memories.list_facts(&user.id, page.offset).await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(facts)))
}
async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<Input>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = body(payload)?;
    validate(&input.title, &input.content).map_err(|_| invalid())?;
    let fact = state
        .memories
        .create_fact(&user.id, &input.title, &input.content)
        .await?;
    Ok((
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(fact),
    ))
}
async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    payload: Result<Json<Edit>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = body(payload)?;
    validate(&input.title, &input.content).map_err(|_| invalid())?;
    if input.version <= 0 {
        return Err(invalid());
    }
    let fact = state
        .memories
        .update_fact(
            &user.id,
            &id(&key)?,
            input.version,
            &input.title,
            &input.content,
        )
        .await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(fact)))
}
async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    payload: Result<Json<Version>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = body(payload)?;
    if input.version <= 0 {
        return Err(invalid());
    }
    state
        .memories
        .delete_fact(&user.id, &id(&key)?, input.version)
        .await?;
    Ok((
        StatusCode::NO_CONTENT,
        [(header::CACHE_CONTROL, "no-store")],
    ))
}
