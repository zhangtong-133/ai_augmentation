use crate::{ApiError, AppState, auth};
use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use personal_ai_storage::replies::{
    Reply, ReplyConfiguration, ReplyOutcome, ReplyStatus, ReplyStore,
};
use serde::Deserialize;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

pub struct ReplyRuntime {
    pub(crate) store: Arc<dyn ReplyStore>,
    enabled: bool,
}

fn configuration() -> ReplyConfiguration {
    ReplyConfiguration {
        model: "builtin-fixture".into(),
        revision: "fixture-v1".into(),
    }
}

fn enabled(value: Option<&str>) -> Result<bool, String> {
    match value {
        None | Some("disabled") => Ok(false),
        Some("fixture") => Ok(true),
        _ => Err("CONVERSATION_REPLY_MODE must be disabled or fixture".into()),
    }
}

impl ReplyRuntime {
    /// 仅允许无网络、无费用的内置夹具；不能通过 URL 或密钥切换真实供应商。
    ///
    /// # Errors
    /// 未知模式拒绝启动。
    pub fn from_env(store: Arc<dyn ReplyStore>) -> Result<Arc<Self>, String> {
        Self::new(
            store,
            std::env::var("CONVERSATION_REPLY_MODE").ok().as_deref(),
        )
    }

    /// # Errors
    /// 仅接受 disabled 或 fixture；此入口同样不允许外部供应商。
    pub fn new(store: Arc<dyn ReplyStore>, mode: Option<&str>) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            store,
            enabled: enabled(mode)?,
        }))
    }

    /// 单轮有界扫描；领取失败或提交结果不明时不执行，终态从不重新派发。
    pub async fn tick(&self) {
        let Ok(items) = self.store.pending_replies().await else {
            tracing::warn!("reply scan unavailable");
            return;
        };
        for item in items {
            if item.status == ReplyStatus::Dispatching {
                if self
                    .store
                    .expire_reply(&item.owner, &item.conversation, &item.request)
                    .await
                    .is_err()
                {
                    tracing::warn!("reply expiration unavailable");
                }
                continue;
            }
            if !self.enabled {
                continue;
            }
            let Ok(reply) = self
                .store
                .claim_reply(&item.owner, &item.conversation, &item.request)
                .await
            else {
                continue;
            };
            // 领取后也不信任旧配置；只处理本版本内置夹具，不转发任何外部调用。
            let outcome = if let Some(context) = reply
                .context
                .filter(|context| context.configuration == configuration())
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let text = context.user_messages.last().map_or("", String::as_str);
                ReplyOutcome::Succeeded(format!("本地测试回复（非模型生成）：{text}"))
            } else {
                ReplyOutcome::Failed
            };
            if self
                .store
                .finish_reply(&item.owner, &item.conversation, &item.request, outcome)
                .await
                .is_err()
            {
                // 不重试生成；已领取状态由期限恢复机制收敛为 unknown。
                tracing::warn!("reply completion unavailable");
            }
        }
    }

    pub async fn run(&self) {
        loop {
            self.tick().await;
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    request_id: Uuid,
    expected_revision: i64,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/conversations/{id}/replies", post(create))
        .route("/api/conversations/{id}/replies/{request}", get(read))
        .route(
            "/api/conversations/{id}/replies/{request}/cancel",
            post(cancel),
        )
}

fn key(value: &str) -> Result<String, ApiError> {
    Uuid::parse_str(value)
        .map(|id| id.to_string())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_reply_id"))
}
fn runtime(state: &AppState) -> Result<&ReplyRuntime, ApiError> {
    state.replies.as_deref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "replies_disabled",
    ))
}
fn public(reply: &Reply) -> serde_json::Value {
    let status = match reply.status {
        ReplyStatus::Queued => "queued",
        ReplyStatus::Dispatching => "dispatching",
        ReplyStatus::Succeeded => "succeeded",
        ReplyStatus::Failed => "failed",
        ReplyStatus::Unknown => "unknown",
        ReplyStatus::Cancelled => "cancelled",
    };
    // 不暴露冻结的系统提示、其他历史正文或内部配置。
    json!({"request_id":reply.request_id,"revision":reply.revision,"status":status,"output":reply.output,"mode":"fixture"})
}
async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    payload: Result<Json<Input>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let Json(input) = payload.map_err(|error| ApiError(error.status(), "invalid_reply"))?;
    let id = key(&id)?;
    if !(1..=100).contains(&input.expected_revision) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_reply_revision"));
    }
    let runtime = runtime(&state)?;
    if !runtime.enabled {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "replies_disabled",
        ));
    }
    let reply = runtime
        .store
        .reserve_reply(
            &user.id,
            &id,
            &input.request_id.to_string(),
            input.expected_revision,
            &configuration(),
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(public(&reply)),
    ))
}
async fn read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, request)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let reply = runtime(&state)?
        .store
        .get_reply(&user.id, &key(&id)?, &key(&request)?)
        .await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(public(&reply))))
}
async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, request)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let reply = runtime(&state)?
        .store
        .cancel_reply(&user.id, &key(&id)?, &key(&request)?)
        .await?;
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(public(&reply))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_explicit_builtin_fixture_is_enabled() {
        assert!(!enabled(None).unwrap());
        assert!(!enabled(Some("disabled")).unwrap());
        assert!(enabled(Some("fixture")).unwrap());
        for mode in ["true", "openai", "http://localhost:8080", ""] {
            assert!(enabled(Some(mode)).is_err());
        }
    }
    #[test]
    fn response_never_serializes_frozen_context() {
        let value = public(&Reply {
            request_id: "id".into(),
            revision: 1,
            status: ReplyStatus::Queued,
            context: Some(personal_ai_storage::replies::ReplyContext {
                system: "secret".into(),
                user_messages: vec!["private".into()],
                first_sequence: 1,
                max_output_tokens: 1024,
                configuration: configuration(),
            }),
            output: None,
        });
        assert!(!value.to_string().contains("secret"));
        assert!(!value.to_string().contains("private"));
        assert_eq!(value["mode"], "fixture");
    }
}
