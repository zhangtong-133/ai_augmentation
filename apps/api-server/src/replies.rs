use crate::{ApiError, AppState, auth, paid_replies::PaidReplies};
use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post},
};
use personal_ai_domain::UserId;
use personal_ai_storage::replies::{
    Reply, ReplyConfiguration, ReplyOutcome, ReplyStatus, ReplyStore,
};
use personal_ai_storage::{
    StorageError,
    reply_budgets::{ReplyDispatchStore, ReplyMoneyReceipt},
};
use serde::Deserialize;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

pub struct ReplyRuntime {
    pub(crate) store: Arc<dyn ReplyStore>,
    enabled: bool,
    budget_store: Option<Arc<dyn ReplyDispatchStore>>,
    paid: Option<PaidReplies>,
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
    /// 从部署配置显式启用固定供应商；缺失、过期或已停用配置拒绝启动。
    /// # Errors
    /// 配置或数据库登记失败。
    pub async fn from_env(store: Arc<dyn ReplyDispatchStore>) -> Result<Arc<Self>, String> {
        let mode = std::env::var("CONVERSATION_REPLY_MODE").ok();
        if !matches!(
            mode.as_deref(),
            None | Some("disabled" | "fixture" | "openai")
        ) {
            return Err("CONVERSATION_REPLY_MODE must be disabled, fixture or openai".into());
        }
        let paid = if mode.as_deref() == Some("openai") {
            Some(crate::paid_replies::from_env(store.clone()).await?)
        } else {
            None
        };
        let enabled = if paid.is_some() {
            true
        } else {
            enabled(mode.as_deref())?
        };
        Ok(Arc::new(Self {
            store: store.clone(),
            enabled,
            budget_store: Some(store),
            paid,
        }))
    }

    /// # Errors
    /// 仅接受 disabled 或 fixture；此入口同样不允许外部供应商。
    pub fn new(store: Arc<dyn ReplyStore>, mode: Option<&str>) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            store,
            enabled: enabled(mode)?,
            budget_store: None,
            paid: None,
        }))
    }

    /// 单轮有界扫描；领取失败或提交结果不明时不执行，终态从不重新派发。
    pub async fn tick(&self) {
        if let Some(paid) = &self.paid {
            paid.executor.tick().await;
        }
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
            if !self.enabled || self.paid.is_some() {
                continue;
            }
            let Ok(queued) = self
                .store
                .get_reply(&item.owner, &item.conversation, &item.request)
                .await
            else {
                continue;
            };
            if queued
                .context
                .as_ref()
                .is_none_or(|context| context.configuration != configuration())
            {
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
    configuration_revision: Option<String>,
    accepted_max_micro: Option<String>,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/conversations/{id}/replies", get(list).post(create))
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
fn with_billing(reply: &Reply, receipts: &[ReplyMoneyReceipt]) -> serde_json::Value {
    let mut value = public(reply);
    if let Some(receipt) = receipts.iter().find(|r| r.request_id == reply.request_id) {
        value["mode"] = json!("openai");
        value["billing"] = json!({"currency":receipt.currency,"reserved_micro":receipt.reserved.to_string(),"charged_micro":receipt.charged.map(|v|v.to_string()),"settlement":receipt.settlement});
    }
    value
}
impl ReplyRuntime {
    async fn receipts(
        &self,
        owner: &UserId,
        conversation: &str,
    ) -> Result<Vec<ReplyMoneyReceipt>, ApiError> {
        match &self.budget_store {
            Some(store) => Ok(store.reply_money_receipts(owner, conversation).await?),
            None => Ok(vec![]),
        }
    }
    async fn response(
        &self,
        owner: &UserId,
        conversation: &str,
        reply: &Reply,
    ) -> Result<serde_json::Value, ApiError> {
        Ok(with_billing(
            reply,
            &self.receipts(owner, conversation).await?,
        ))
    }
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let runtime = runtime(&state)?;
    let id = key(&id)?;
    let items = runtime.store.list_replies(&user.id, &id).await?;
    let receipts = runtime.receipts(&user.id, &id).await?;
    let mut enabled = runtime.enabled
        && runtime
            .paid
            .as_ref()
            .is_none_or(|paid| !paid.executor.is_halted());
    if let (Some(paid), Some(store)) = (&runtime.paid, &runtime.budget_store) {
        match store
            .check_reply_configuration(&paid.configuration, &paid.budget)
            .await
        {
            Ok(()) => {}
            Err(StorageError::Conflict(_) | StorageError::NotFound) => enabled = false,
            Err(error) => return Err(error.into()),
        }
    }
    let mode = if runtime.paid.is_some() {
        "openai"
    } else {
        "fixture"
    };
    let quote = runtime.paid.as_ref().map(PaidReplies::quote);
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(
            json!({"enabled":enabled,"mode":mode,"quote":quote,"items":items.iter().map(|r|with_billing(r, &receipts)).collect::<Vec<_>>()}),
        ),
    ))
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
    let request = input.request_id.to_string();
    // 已提交请求的重放不依赖当前价格或启用模式，不会重新预留。
    let existing = runtime.store.get_reply(&user.id, &id, &request).await;
    let reply = match existing {
        Ok(reply) => {
            if reply.revision != input.expected_revision {
                return Err(ApiError(StatusCode::CONFLICT, "reply_request_reused"));
            }
            reply
        }
        Err(StorageError::NotFound) => {
            if !runtime.enabled
                || runtime
                    .paid
                    .as_ref()
                    .is_some_and(|paid| paid.executor.is_halted())
            {
                return Err(ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "replies_disabled",
                ));
            }
            if let (Some(paid), Some(store)) = (&runtime.paid, &runtime.budget_store) {
                if input.configuration_revision.as_deref() != Some(&paid.configuration.revision)
                    || input.accepted_max_micro.as_deref()
                        != Some(paid.reservation.to_string().as_str())
                {
                    return Err(ApiError(
                        StatusCode::CONFLICT,
                        "reply_quote_changed_or_not_accepted",
                    ));
                }
                store
                    .reserve_active_budgeted_reply(
                        &user.id,
                        &id,
                        &request,
                        input.expected_revision,
                        &paid.configuration,
                        paid.planner.clone(),
                    )
                    .await?
            } else {
                if input.configuration_revision.is_some() || input.accepted_max_micro.is_some() {
                    return Err(ApiError(StatusCode::CONFLICT, "reply_mode_changed"));
                }
                runtime
                    .store
                    .reserve_reply(
                        &user.id,
                        &id,
                        &request,
                        input.expected_revision,
                        &configuration(),
                    )
                    .await?
            }
        }
        Err(error) => return Err(error.into()),
    };
    Ok((
        StatusCode::ACCEPTED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(runtime.response(&user.id, &id, &reply).await?),
    ))
}
async fn read(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, request)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    let runtime = runtime(&state)?;
    let id = key(&id)?;
    let reply = runtime
        .store
        .get_reply(&user.id, &key(&id)?, &key(&request)?)
        .await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(runtime.response(&user.id, &id, &reply).await?),
    ))
}
async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, request)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let user = auth::current_user(&state, &headers).await?;
    auth::mutation_guard(&headers)?;
    let runtime = runtime(&state)?;
    let id = key(&id)?;
    let reply = runtime
        .store
        .cancel_reply(&user.id, &key(&id)?, &key(&request)?)
        .await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(runtime.response(&user.id, &id, &reply).await?),
    ))
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

#[cfg(test)]
#[path = "paid_reply_tests.rs"]
mod paid_tests;
