use crate::{ApiError, AppState, auth};
use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use personal_ai_storage::messages::{
    MessageCache, MessageSnapshot, MessageStore, validate_content,
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    request_id: Uuid,
    content: String,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/api/conversations/{id}/messages", get(list).post(append))
}
fn id(value: &str) -> Result<String, ApiError> {
    Uuid::parse_str(value)
        .map(|value| value.to_string())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_conversation"))
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let key = id(&key)?;
    // 每次读取都先核验权威归属、删除状态及版本，Redis 不能授权访问。
    let revision = state.messages.message_revision(&user.id, &key).await?;
    if let Some(cache) = &state.message_cache
        && let Ok(Some(snapshot)) = cache.get(&user.id, &key).await
        && !snapshot.deleted
        && snapshot.revision == revision
    {
        return Ok(([(header::CACHE_CONTROL, "no-store")], Json(snapshot)));
    }
    let snapshot = state.messages.message_snapshot(&user.id, &key).await?;
    if let Some(cache) = &state.message_cache {
        let _ = cache.put(&user.id, &key, &snapshot).await;
    }
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(snapshot)))
}
async fn append(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
    payload: Result<Json<Input>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let Json(input) = payload.map_err(|error| ApiError(error.status(), "invalid_message"))?;
    validate_content(&input.content)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_message"))?;
    let message = state
        .messages
        .append_message(
            &user.id,
            &id(&key)?,
            &input.request_id.to_string(),
            &input.content,
        )
        .await?;
    // 成功仅代表 PostgreSQL 已提交；缓存由读取重建，不影响消息写入结果。
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(message)))
}

/// 清理任务可重复执行；数据库记录保证进程重启后继续，缓存失败不确认。
pub async fn reconcile_message_deletions(store: &dyn MessageStore, cache: &dyn MessageCache) {
    let Ok(items) = store.pending_cache_deletions().await else {
        return;
    };
    for item in items {
        let snapshot = MessageSnapshot {
            revision: item.revision,
            deleted: true,
            messages: vec![],
        };
        if cache
            .put(&item.owner, &item.conversation, &snapshot)
            .await
            .is_ok()
        {
            let _ = store.acknowledge_cache_deletion(&item).await;
        }
    }
}

/// # Errors
/// 开关或 Redis URL 无效时拒绝启动，不输出凭据。
pub fn message_cache_from_env() -> Result<Option<Arc<dyn MessageCache>>, String> {
    cache_config(|key| std::env::var(key).ok())
}
fn cache_config(
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<Arc<dyn MessageCache>>, String> {
    match env("MESSAGE_CACHE_ENABLED").as_deref() {
        None | Some("false") => Ok(None),
        Some("true") => {
            let url = env("REDIS_URL").ok_or("REDIS_URL is required for message cache")?;
            let cache = personal_ai_storage_redis::RedisMessageCache::new(&url)
                .map_err(|_| "invalid message cache configuration")?;
            Ok(Some(Arc::new(cache)))
        }
        _ => Err("MESSAGE_CACHE_ENABLED must be true or false".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_is_explicit_and_invalid_configuration_fails() {
        assert!(cache_config(|_| None).unwrap().is_none());
        assert!(
            cache_config(|key| (key == "MESSAGE_CACHE_ENABLED").then(|| "true".into())).is_err()
        );
        assert!(cache_config(|_| Some("invalid".into())).is_err());
    }
}
