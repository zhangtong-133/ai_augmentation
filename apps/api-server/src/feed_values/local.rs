use super::{
    ApiError, AppState, Arc, Deserialize, Empty, HeaderMap, IntoResponse, Json, JsonRejection,
    Path, Query, QueryRejection, Response, State, StorageError, StorageResult, UserId,
    ValueApproval, ValuePricing, ValueQuotePlanner, ValueSnapshot, auth, body, error, full, id,
    invalid, json, runtime,
};
use personal_ai_llm::local::LocalTarget;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preview {
    id: String,
    endpoint: String,
    model: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Approval {
    digest: String,
    acknowledge_sharing: bool,
    acknowledge_local_compute: bool,
}
fn enabled() -> bool {
    std::env::var("RSS_LOCAL_ENABLED").ok().as_deref() == Some("true")
}
fn guard() -> Result<(), ApiError> {
    if enabled() {
        Ok(())
    } else {
        Err(error(StorageError::Unavailable(
            "local scoring disabled".into(),
        )))
    }
}
pub(super) async fn config(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<Empty>, QueryRejection>,
) -> Result<Response, ApiError> {
    auth::current_user(&state, &headers).await?;
    query.map_err(|_| invalid())?;
    Ok(Json(json!({"local_enabled":enabled(),"execution_mode":"local_only"})).into_response())
}
struct Planner(LocalTarget);
impl ValueQuotePlanner for Planner {
    fn quote(&self, _: &UserId, _: &str, snapshot: &ValueSnapshot) -> StorageResult<ValuePricing> {
        let time = i64::try_from(snapshot.as_of_unix_ms)
            .ok()
            .and_then(|t| t.checked_add(300_000))
            .ok_or_else(|| StorageError::InvalidData("invalid local deadline".into()))?;
        Ok(ValuePricing::Local {
            endpoint: self.0.endpoint().into(),
            model: self.0.model().into(),
            profile: personal_ai_agent_core::feed_value_local::LOCAL_VALUE_PROFILE.into(),
            valid_until_unix_ms: time,
        })
    }
}
pub(super) async fn preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<Empty>, QueryRejection>,
    input: Result<Json<Preview>, JsonRejection>,
) -> Result<Response, ApiError> {
    let owner = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    query.map_err(|_| invalid())?;
    guard()?;
    let input = body(input)?;
    let target = LocalTarget::new(&input.endpoint, &input.model).map_err(|_| invalid())?;
    let saved = runtime(&state)?
        .preview_feed_value(&owner.id, &id(&input.id)?, Arc::new(Planner(target)))
        .await
        .map_err(error)?;
    if !matches!(saved.pricing, ValuePricing::Local { .. }) {
        return Err(error(StorageError::Conflict(
            "different scoring mode".into(),
        )));
    }
    Ok(Json(full(&owner.id, &saved)?).into_response())
}
pub(super) async fn approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    query: Result<Query<Empty>, QueryRejection>,
    input: Result<Json<Approval>, JsonRejection>,
) -> Result<Response, ApiError> {
    let owner = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    query.map_err(|_| invalid())?;
    guard()?;
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
                acknowledge_subscription_usage: false,
                acknowledge_local_compute: input.acknowledge_local_compute,
            },
        )
        .await
        .map_err(error)?;
    Ok(Json(full(&owner.id, &saved)?).into_response())
}
