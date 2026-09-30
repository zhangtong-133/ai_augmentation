//! 内部预算回复执行器。应用入口必须显式组装；不读取环境变量或自动启动。
use crate::BoxFuture;
use personal_ai_storage::{
    replies::{PendingReply, ReplyConfiguration, ReplyContext, ReplyOutcome, ReplyStatus},
    reply_budgets::{ReplyBudget, ReplyDispatchStore, ReplyUsage},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplySendError {
    /// 尚未发送：配置已停用、不匹配或请求/预算无效。
    InvalidConfiguration,
    /// 发送后结果不确定；不能推断未计费，禁止自动重发。
    Unknown,
    /// 响应不能作为有效回复或可信结算依据，保留全部预留。
    InvalidResponse,
    /// 模型/服务等级/费用上界契约异常；执行器必须持久化停用配置。
    ContractViolation,
}
#[derive(Debug, PartialEq, Eq)]
pub struct ReplyCompletion {
    pub content: String,
    /// 缺失或不完整的 usage 不允许退差额。
    pub usage: Option<ReplyUsage>,
}

/// 实现每次调用至多一次网络发送，禁止 SDK、HTTP 或代理自动重试。
pub trait ReplySender: Send + Sync {
    fn send<'a>(
        &'a self,
        context: &'a ReplyContext,
        budget: &'a ReplyBudget,
    ) -> BoxFuture<'a, Result<ReplyCompletion, ReplySendError>>;
}

pub struct BudgetedReplyExecutor {
    store: Arc<dyn ReplyDispatchStore>,
    sender: Arc<dyn ReplySender>,
    configuration: ReplyConfiguration,
    halted: AtomicBool,
}

impl BudgetedReplyExecutor {
    #[must_use]
    pub fn new(
        store: Arc<dyn ReplyDispatchStore>,
        sender: Arc<dyn ReplySender>,
        configuration: ReplyConfiguration,
    ) -> Self {
        Self {
            store,
            sender,
            configuration,
            halted: AtomicBool::new(false),
        }
    }

    /// 本实例观察到供应商契约异常后，立即停止接受新派发。
    #[must_use]
    pub fn is_halted(&self) -> bool {
        self.halted.load(Ordering::SeqCst)
    }

    /// 有界扫描，已领取请求仅做过期恢复；不重试发送或重新排队。
    pub async fn tick(&self) {
        // 停用写入失败时，本实例保持关闭；后续轮次重试持久化，不再发送。
        if self.halted.load(Ordering::SeqCst) {
            self.persist_disable().await;
        }
        let Ok(items) = self
            .store
            .pending_budgeted_replies(&self.configuration)
            .await
        else {
            tracing::warn!("budgeted reply scan unavailable");
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
                    tracing::warn!("budgeted reply expiration unavailable");
                }
                continue;
            }
            if !self.halted.load(Ordering::SeqCst) {
                self.dispatch(&item).await;
            }
        }
    }

    async fn persist_disable(&self) {
        if self
            .store
            .disable_reply_configuration(&self.configuration.revision)
            .await
            .is_err()
        {
            tracing::error!(configuration_revision = %self.configuration.revision, "reply configuration disable persistence failed; executor halted");
        }
    }

    async fn dispatch(&self, item: &PendingReply) {
        // 不领取其他执行器的配置或旧夹具请求。
        let Ok(queued) = self
            .store
            .get_reply(&item.owner, &item.conversation, &item.request)
            .await
        else {
            return;
        };
        if queued
            .context
            .as_ref()
            .is_none_or(|c| c.configuration != self.configuration)
        {
            return;
        }
        let Ok(claim) = self
            .store
            .claim_budgeted_reply(&item.owner, &item.conversation, &item.request)
            .await
        else {
            return;
        };
        let Some(context) = claim.reply.context else {
            return;
        };
        // 提交已确认。先执行过期收敛，再读取取消/删除状态，最后复核持久化配置。
        let active = self
            .store
            .expire_reply(&item.owner, &item.conversation, &item.request)
            .await;
        if !matches!(active, Ok(ref reply) if reply.status == ReplyStatus::Dispatching) {
            return;
        }
        if self.halted.load(Ordering::SeqCst)
            || context.configuration != self.configuration
            || self
                .store
                .check_reply_configuration(&context.configuration, &claim.budget)
                .await
                .is_err()
        {
            self.finish(item, ReplyOutcome::Unknown, None).await;
            return;
        }
        let result = tokio::time::timeout(
            Duration::from_secs(35),
            self.sender.send(&context, &claim.budget),
        )
        .await;
        let (outcome, usage) = match result {
            Ok(Ok(completion)) => {
                if completion.usage.is_some_and(|usage| {
                    usage.input_tokens > claim.budget.input_token_bound
                        || usage.output_tokens > claim.budget.output_token_bound
                }) {
                    self.halt().await;
                    (ReplyOutcome::Unknown, None)
                } else if completion.content.trim().is_empty()
                    || completion.content.len() > 16384
                    || completion.content.contains('\0')
                {
                    (ReplyOutcome::Failed, None)
                } else {
                    (
                        ReplyOutcome::Succeeded(completion.content),
                        completion.usage,
                    )
                }
            }
            Ok(Err(ReplySendError::ContractViolation)) => {
                self.halt().await;
                (ReplyOutcome::Unknown, None)
            }
            Ok(Err(ReplySendError::InvalidConfiguration | ReplySendError::InvalidResponse)) => {
                (ReplyOutcome::Failed, None)
            }
            Ok(Err(ReplySendError::Unknown)) | Err(_) => (ReplyOutcome::Unknown, None),
        };
        self.finish(item, outcome, usage).await;
    }

    async fn halt(&self) {
        self.halted.store(true, Ordering::SeqCst);
        tracing::error!(configuration_revision = %self.configuration.revision,
            "reply provider contract violation; executor halted; billing review required");
        self.persist_disable().await;
    }

    async fn finish(&self, item: &PendingReply, outcome: ReplyOutcome, usage: Option<ReplyUsage>) {
        if self
            .store
            .finish_budgeted_reply(
                &item.owner,
                &item.conversation,
                &item.request,
                outcome,
                usage,
            )
            .await
            .is_err()
        {
            // 不重发生成。持久化失败保留派发状态和全部预留，由期限恢复收敛。
            tracing::warn!("budgeted reply completion unavailable; no redispatch");
        }
    }
}
