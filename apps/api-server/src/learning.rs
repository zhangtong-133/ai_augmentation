use crate::{ApiError, AppState, auth};
use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Path, Query, Request, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use personal_ai_storage::{
    StorageError,
    learning::{
        AssessmentInput, LearningPlanInput, LearningStore, SavedLearningPlan, SkillInput,
        TrainingEvidenceBody, TrainingOutcome, TrainingResultInput,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/learning/snapshot", get(snapshot))
        .route("/api/learning/progress", get(progress))
        .route(
            "/api/learning/skills/{id}",
            put(save_skill).delete(delete_skill),
        )
        .route("/api/learning/assessments", post(assess))
        .route("/api/learning/plans", get(history).post(generate))
        .route("/api/learning/plans/{id}", get(detail).delete(delete_plan))
        .route(
            "/api/learning/plans/{plan}/tasks/{task}/result",
            post(result),
        )
        .route(
            "/api/learning/plans/{plan}/tasks/{task}/evidence",
            post(save_evidence)
                .layer(DefaultBodyLimit::max(64 * 1024))
                .delete(delete_evidence),
        )
        .route(
            "/api/learning/plans/{plan}/tasks/{task}/evidence/review",
            post(save_review),
        )
        .route(
            "/api/learning/plans/{plan}/tasks/{task}/evidence/review/confirm",
            post(confirm_review),
        )
        .layer(middleware::from_fn(no_store))
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
struct Revision {
    revision: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Skill {
    revision: String,
    name: String,
    enabled: bool,
    prerequisite_ids: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Assessment {
    request_id: String,
    expected_revision: String,
    skill_id: String,
    skill_revision: String,
    score: u8,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Generate {
    request_id: String,
    expected_revision: String,
    budget_minutes: u16,
    goal_skill_ids: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Completed,
    Cancelled,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultInput {
    request_id: String,
    outcome: Outcome,
    note: String,
    actual_minutes: u16,
}
async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
fn invalid() -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, "invalid_learning")
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
    if n < 0 || n.to_string() != value {
        return Err(invalid());
    }
    u64::try_from(n).map_err(|_| invalid())
}
fn payload<T>(value: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    value
        .map(|Json(v)| v)
        .map_err(|e| ApiError(e.status(), "invalid_learning"))
}
fn page(value: Result<Query<Page>, QueryRejection>) -> Result<Option<String>, ApiError> {
    Ok(value.map_err(|_| invalid())?.0.after)
}
fn runtime(state: &AppState) -> Result<&dyn LearningStore, ApiError> {
    state.learning.as_deref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "learning_unavailable",
    ))
}
#[allow(clippy::needless_pass_by_value)]
fn error(value: StorageError) -> ApiError {
    match value {
        StorageError::NotFound => ApiError(StatusCode::NOT_FOUND, "learning_not_found"),
        StorageError::InvalidData(_) => invalid(),
        StorageError::Conflict(_) => ApiError(StatusCode::CONFLICT, "learning_conflict"),
        StorageError::Unavailable(_) => {
            ApiError(StatusCode::SERVICE_UNAVAILABLE, "learning_unavailable")
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
                        || key.ends_with("_revision"))
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
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "learning_unavailable"))?;
    exact(&mut value);
    Ok(Json(value).into_response())
}
fn plan_output(saved: &SavedLearningPlan) -> Result<Response, ApiError> {
    #[derive(Serialize)]
    struct Detail<'a> {
        #[serde(flatten)]
        saved: &'a SavedLearningPlan,
        evidence_reviews: Vec<personal_ai_storage::learning::evidence::EvidenceReview<'a>>,
    }
    output(&Detail {
        saved,
        evidence_reviews: saved.evidence_reviews(),
    })
}
async fn snapshot(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    output(
        &runtime(&state)?
            .learning_snapshot(&user.id)
            .await
            .map_err(error)?,
    )
}
async fn save_skill(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Result<Json<Skill>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    output(
        &runtime(&state)?
            .save_skill(
                &user.id,
                &id(&key)?,
                revision(&input.revision)?,
                &SkillInput {
                    name: input.name,
                    enabled: input.enabled,
                    prerequisite_ids: input.prerequisite_ids,
                },
            )
            .await
            .map_err(error)?,
    )
}
async fn delete_skill(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Result<Json<Revision>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    runtime(&state)?
        .delete_skill(&user.id, &id(&key)?, revision(&input.revision)?)
        .await
        .map_err(error)?;
    output(&json!({"deleted":true}))
}
async fn assess(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<Assessment>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    output(
        &runtime(&state)?
            .record_assessment(
                &user.id,
                &id(&input.request_id)?,
                &AssessmentInput {
                    expected_revision: revision(&input.expected_revision)?,
                    skill_id: id(&input.skill_id)?,
                    skill_revision: revision(&input.skill_revision)?,
                    score: input.score,
                },
            )
            .await
            .map_err(error)?,
    )
}
async fn generate(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<Generate>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    let saved = runtime(&state)?
        .create_learning_plan(
            &user.id,
            &id(&input.request_id)?,
            &LearningPlanInput {
                expected_revision: revision(&input.expected_revision)?,
                budget_minutes: input.budget_minutes,
                goal_skill_ids: input.goal_skill_ids,
            },
        )
        .await
        .map_err(error)?;
    let mut response = plan_output(&saved)?;
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
            .list_learning_plans(&user.id, after.as_deref())
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
    plan_output(
        &runtime(&state)?
            .get_learning_plan(&user.id, &id(&key)?)
            .await
            .map_err(error)?,
    )
}
async fn delete_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Result<Json<Empty>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    payload(body)?;
    runtime(&state)?
        .delete_learning_plan(&user.id, &id(&key)?)
        .await
        .map_err(error)?;
    output(&json!({"deleted":true}))
}
async fn result(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((plan, task)): Path<(String, String)>,
    body: Result<Json<ResultInput>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    plan_output(
        &runtime(&state)?
            .record_training_result(
                &user.id,
                &id(&plan)?,
                &id(&task)?,
                &TrainingResultInput {
                    request_id: id(&input.request_id)?,
                    outcome: match input.outcome {
                        Outcome::Completed => TrainingOutcome::Completed,
                        Outcome::Cancelled => TrainingOutcome::Cancelled,
                    },
                    note: input.note,
                    actual_minutes: input.actual_minutes,
                },
            )
            .await
            .map_err(error)?,
    )
}

async fn progress(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<Empty>, QueryRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    query.map_err(|_| invalid())?;
    output(
        &runtime(&state)?
            .learning_progress(&user.id)
            .await
            .map_err(error)?,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceInput {
    request_id: String,
    body: TrainingEvidenceBody,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceDelete {
    request_id: String,
}
async fn save_evidence(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((plan, task)): Path<(String, String)>,
    body: Result<Json<EvidenceInput>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    plan_output(
        &runtime(&state)?
            .save_training_evidence(
                &user.id,
                &id(&plan)?,
                &id(&task)?,
                &id(&input.request_id)?,
                &input.body,
            )
            .await
            .map_err(error)?,
    )
}
async fn delete_evidence(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((plan, task)): Path<(String, String)>,
    body: Result<Json<EvidenceDelete>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    plan_output(
        &runtime(&state)?
            .delete_training_evidence(&user.id, &id(&plan)?, &id(&task)?, &id(&input.request_id)?)
            .await
            .map_err(error)?,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewInput {
    request_id: String,
    evidence_request_id: String,
    body: personal_ai_storage::learning::review::ReviewBody,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewConfirmation {
    request_id: String,
    review_request_id: String,
    expected_revision: String,
    score: u8,
}
async fn save_review(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((plan, task)): Path<(String, String)>,
    body: Result<Json<ReviewInput>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    plan_output(
        &runtime(&state)?
            .save_training_review(
                &user.id,
                &id(&plan)?,
                &id(&task)?,
                &personal_ai_storage::learning::review::ReviewInput {
                    request_id: id(&input.request_id)?,
                    evidence_request_id: id(&input.evidence_request_id)?,
                    body: input.body,
                },
            )
            .await
            .map_err(error)?,
    )
}
async fn confirm_review(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((plan, task)): Path<(String, String)>,
    body: Result<Json<ReviewConfirmation>, JsonRejection>,
) -> Result<Response, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    plan_output(
        &runtime(&state)?
            .confirm_training_review(
                &user.id,
                &id(&plan)?,
                &id(&task)?,
                &personal_ai_storage::learning::review::ReviewConfirmation {
                    request_id: id(&input.request_id)?,
                    review_request_id: id(&input.review_request_id)?,
                    expected_revision: revision(&input.expected_revision)?,
                    score: input.score,
                },
            )
            .await
            .map_err(error)?,
    )
}
