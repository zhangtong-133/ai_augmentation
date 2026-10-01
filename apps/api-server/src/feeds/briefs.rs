use super::{
    ApiError, AppState, Deserialize, Empty, HeaderMap, Json, JsonRejection, Page, Path, Query,
    QueryRejection, Response, Router, State, StatusCode, auth, error, get, id, invalid, json,
    output, page, payload, runtime,
};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/feed-brief-preferences", get(preferences).put(save))
        .route("/api/feed-briefs", get(history).post(create))
        .route("/api/feed-briefs/{id}", get(detail).delete(delete))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    revision: String,
    keywords: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Generate {
    request_id: String,
    preference_revision: String,
}
fn version(value: &str) -> Result<u64, ApiError> {
    let n = value.parse::<i64>().map_err(|_| invalid())?;
    if n < 0 || n.to_string() != value {
        return Err(invalid());
    }
    u64::try_from(n).map_err(|_| invalid())
}
async fn preferences(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    output(
        &runtime(&state)?
            .briefs
            .get_brief_preferences(&user.id)
            .await
            .map_err(error)?,
    )
}
async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<Preferences>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    output(
        &runtime(&state)?
            .briefs
            .save_brief_preferences(&user.id, version(&input.revision)?, &input.keywords)
            .await
            .map_err(error)?,
    )
}
async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<Generate>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    let saved = runtime(&state)?
        .briefs
        .create_brief(
            &user.id,
            &id(&input.request_id)?,
            version(&input.preference_revision)?,
        )
        .await
        .map_err(error)?;
    let mut response = output(&saved)?;
    *response.status_mut() = StatusCode::CREATED;
    Ok(response)
}
async fn history(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<Page>, QueryRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let after = page(query)?.as_deref().map(id).transpose()?;
    output(
        &runtime(&state)?
            .briefs
            .list_briefs(&user.id, after.as_deref())
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
            .briefs
            .get_brief(&user.id, &id(&key)?)
            .await
            .map_err(error)?,
    )
}
async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Result<Json<Empty>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    payload(body)?;
    runtime(&state)?
        .briefs
        .delete_brief(&user.id, &id(&key)?)
        .await
        .map_err(error)?;
    output(&json!({"deleted":true}))
}
