use super::{
    ApiError, AppState, Deserialize, Empty, HeaderMap, Json, JsonRejection, Path, Query,
    QueryRejection, Response, Router, State, auth, error, get, id, invalid, output, payload, post,
    revision, runtime,
};
use personal_ai_storage::learning::model_authorization::{ModelApproval, ModelAuthorizationInput};
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/learning/plans/{plan}/tasks/{task}/evidence/model-authorizations",
            post(create),
        )
        .route("/api/learning/model-authorizations", get(list))
        .route("/api/learning/model-authorizations/{request}", get(detail))
        .route(
            "/api/learning/model-authorizations/{request}/approve",
            post(approve),
        )
        .route(
            "/api/learning/model-authorizations/{request}/cancel",
            post(cancel),
        )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    request_id: String,
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
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    after: Option<String>,
}
async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((plan, task)): Path<(String, String)>,
    body: Result<Json<Create>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    output(
        &runtime(&state)?
            .create_model_authorization(
                &user.id,
                &id(&plan)?,
                &id(&task)?,
                &ModelAuthorizationInput {
                    request_id: id(&input.request_id)?,
                    connection_id: id(&input.connection_id)?,
                    connection_revision: revision(&input.connection_revision)?,
                    model: input.model,
                },
            )
            .await
            .map_err(error)?,
    )
}
async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request): Path<String>,
    query: Result<Query<Empty>, QueryRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    query.map_err(|_| invalid())?;
    output(
        &runtime(&state)?
            .get_model_authorization(&user.id, &id(&request)?)
            .await
            .map_err(error)?,
    )
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<Page>, QueryRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let Query(page) = query.map_err(|_| invalid())?;
    let after = page.after.as_deref().map(id).transpose()?;
    output(
        &runtime(&state)?
            .list_model_authorizations(&user.id, after.as_deref())
            .await
            .map_err(error)?,
    )
}
async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request): Path<String>,
    body: Result<Json<Approval>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    output(
        &runtime(&state)?
            .approve_model_authorization(
                &user.id,
                &id(&request)?,
                &ModelApproval {
                    digest: input.digest,
                    acknowledge_sharing: input.acknowledge_sharing,
                    acknowledge_subscription_usage: input.acknowledge_subscription_usage,
                },
            )
            .await
            .map_err(error)?,
    )
}
async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request): Path<String>,
    body: Result<Json<Empty>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    payload(body)?;
    output(
        &runtime(&state)?
            .cancel_model_authorization(&user.id, &id(&request)?)
            .await
            .map_err(error)?,
    )
}
