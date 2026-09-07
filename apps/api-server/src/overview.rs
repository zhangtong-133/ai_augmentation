use crate::{ApiError, AppState, auth};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

const DAY_MS: i64 = 86_400_000;

fn day_window(now_ms: i64) -> (i64, i64) {
    let start = now_ms.div_euclid(DAY_MS) * DAY_MS;
    (start, start + DAY_MS)
}

pub(super) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|t| i64::try_from(t.as_millis()).ok())
        .ok_or(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "clock_error"))?;
    let (start, end) = day_window(now_ms);
    let knowledge = state.documents.document_stats(&user.id, start, end).await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({
            "timezone": "UTC",
            "day_start_unix_ms": start,
            "day_end_unix_ms": end,
            "generated_at_unix_ms": now_ms,
            "knowledge": knowledge,
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_day_changes_at_midnight() {
        assert_eq!(day_window(DAY_MS - 1), (0, DAY_MS));
        assert_eq!(day_window(DAY_MS), (DAY_MS, 2 * DAY_MS));
    }
}
