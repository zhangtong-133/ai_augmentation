use crate::{ApiError, AppState, auth};
use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use personal_ai_storage::{
    StorageError,
    schedules::{NewSchedule, ScheduleApproval, ScheduleDeliveryStore, ScheduleStore},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub trait SchedulerStore: ScheduleStore + ScheduleDeliveryStore {}
impl<T: ScheduleStore + ScheduleDeliveryStore> SchedulerStore for T {}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/schedules", get(list).post(create))
        .route("/api/schedules/{id}", get(read))
        .route("/api/schedules/{id}/approve", post(approve))
        .route("/api/schedules/{id}/cancel", post(cancel))
        .route("/api/reminders", get(reminders))
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    after: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    request_id: String,
    title: String,
    body: String,
    run_at_unix_ms: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    digest: String,
    accepted_run_at_unix_ms: String,
    accepted_max_runs: u32,
    accepted_amount_micro: String,
    acknowledge_schedule: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
fn invalid() -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, "invalid_schedule")
}
fn id(value: &str) -> Result<String, ApiError> {
    uuid::Uuid::parse_str(value)
        .map(|v| v.to_string())
        .map_err(|_| invalid())
}
fn number(value: &str) -> Result<i64, ApiError> {
    let number = value.parse::<i64>().map_err(|_| invalid())?;
    if number < 0 || number.to_string() != value {
        return Err(invalid());
    }
    Ok(number)
}
fn payload<T>(value: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    value
        .map(|Json(v)| v)
        .map_err(|e| ApiError(e.status(), "invalid_schedule"))
}
fn store(state: &AppState) -> Result<&dyn SchedulerStore, ApiError> {
    state.schedules.as_deref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "scheduler_unavailable",
    ))
}
#[allow(clippy::needless_pass_by_value)]
fn error(error: StorageError) -> ApiError {
    match error {
        StorageError::NotFound => ApiError(StatusCode::NOT_FOUND, "schedule_not_found"),
        StorageError::InvalidData(_) => invalid(),
        StorageError::Conflict(_) => ApiError(StatusCode::CONFLICT, "schedule_conflict"),
        StorageError::Unavailable(_) => {
            ApiError(StatusCode::SERVICE_UNAVAILABLE, "scheduler_unavailable")
        }
    }
}
// 内部计数保持数字；金额与 UTC 毫秒统一为精确字符串，租约不在公开类型中。
fn view(value: &impl Serialize) -> Result<Value, ApiError> {
    fn exact(v: &mut Value) {
        match v {
            Value::Object(fields) => {
                for (key, value) in fields {
                    if (key.ends_with("_unix_ms") || key == "amount_micro") && value.is_number() {
                        *value = Value::String(value.to_string());
                    } else {
                        exact(value);
                    }
                }
            }
            Value::Array(values) => {
                for v in values {
                    exact(v);
                }
            }
            _ => (),
        }
    }
    let mut value = serde_json::to_value(value)
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "scheduler_unavailable"))?;
    exact(&mut value);
    Ok(value)
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    page: Result<Query<Page>, QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let Query(page) = page.map_err(|_| invalid())?;
    let after = page.after.as_deref().map(id).transpose()?;
    let data = store(&state)?
        .list_schedules(&user.id, after.as_deref())
        .await
        .map_err(error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(view(&data)?)))
}
async fn reminders(
    State(state): State<AppState>,
    headers: HeaderMap,
    page: Result<Query<Page>, QueryRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let Query(page) = page.map_err(|_| invalid())?;
    let after = page.after.as_deref().map(id).transpose()?;
    let data = store(&state)?
        .list_schedule_reminders(&user.id, after.as_deref())
        .await
        .map_err(error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(view(&data)?)))
}
async fn read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let data = store(&state)?
        .get_schedule(&user.id, &id(&key)?)
        .await
        .map_err(error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(view(&data)?)))
}
async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<Input>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    let input = NewSchedule {
        request_id: id(&input.request_id)?,
        title: input.title,
        body: input.body,
        run_at_unix_ms: number(&input.run_at_unix_ms)?,
    };
    let data = store(&state)?
        .create_schedule(&user.id, &input)
        .await
        .map_err(error)?;
    Ok((
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(view(&data)?),
    ))
}
async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Result<Json<Approval>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let input = payload(body)?;
    let approval = ScheduleApproval {
        digest: input.digest,
        accepted_run_at_unix_ms: number(&input.accepted_run_at_unix_ms)?,
        accepted_max_runs: input.accepted_max_runs,
        accepted_amount_micro: number(&input.accepted_amount_micro)?,
        acknowledge_schedule: input.acknowledge_schedule,
    };
    let data = store(&state)?
        .approve_schedule(&user.id, &id(&key)?, &approval)
        .await
        .map_err(error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(view(&data)?)))
}
async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    body: Result<Json<Empty>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    payload(body)?;
    let data = store(&state)?
        .cancel_schedule(&user.id, &id(&key)?)
        .await
        .map_err(error)?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(view(&data)?)))
}
