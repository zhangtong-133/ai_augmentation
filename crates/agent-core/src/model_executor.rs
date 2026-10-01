//! 显式两阶段授权驱动的一次性执行器；不扫描、不自动批准下一阶段、不重发。
use crate::{
    BoxFuture,
    model_answer::plan_model_answer,
    model_plan::{AgentRequestIdentity, QuoteWindow, quote_model_planning},
    reply_executor::{ReplyCompletion, ReplySendError},
};
use personal_ai_domain::{ConversationId, UserId};
use personal_ai_llm::{ChatRequest, Embedding};
use personal_ai_storage::{
    StorageError, StorageResult,
    messages::MessageStore,
    model_agents::{
        AgentQuoteApproval, ModelCallBudget, ModelPlanningConfiguration, ModelPlanningOutcome,
        ModelPlanningRequest, ModelPlanningStore,
    },
    model_execution::{
        ModelExecutionConfiguration, ModelExecutionOutcome, ModelExecutionRequest,
        ModelExecutionStore, RetrievedChunk,
    },
    reply_budgets::ReplyUsage,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelChatStage {
    Planning,
    Answer,
}
pub struct ModelEmbeddingCompletion {
    pub embedding: Embedding,
    pub usage: Option<ReplyUsage>,
}
/// 每次调用最多一次付费 HTTP 请求；严格匹配冻结预算并离线计数。
pub trait ModelAgentProvider: Send + Sync {
    fn chat<'a>(
        &'a self,
        stage: ModelChatStage,
        request: &'a ChatRequest,
        budget: &'a ModelCallBudget,
    ) -> BoxFuture<'a, Result<ReplyCompletion, ReplySendError>>;
    fn embed<'a>(
        &'a self,
        query: &'a str,
        budget: &'a ModelCallBudget,
    ) -> BoxFuture<'a, Result<ModelEmbeddingCompletion, ReplySendError>>;
}
/// 只消费已经生成的向量，不得再次调用模型或重复登记工具次数。
pub trait ModelRetriever: Send + Sync {
    fn retrieve<'a>(
        &'a self,
        owner: &'a UserId,
        embedding: &'a Embedding,
        limit: usize,
    ) -> BoxFuture<'a, StorageResult<Vec<RetrievedChunk>>>;
}
pub trait ModelAgentStore: ModelPlanningStore + ModelExecutionStore + MessageStore {}
impl<T: ModelPlanningStore + ModelExecutionStore + MessageStore> ModelAgentStore for T {}

pub struct ModelAgentExecutor {
    store: Arc<dyn ModelAgentStore>,
    provider: Arc<dyn ModelAgentProvider>,
    retriever: Arc<dyn ModelRetriever>,
    planning: ModelPlanningConfiguration,
    execution: ModelExecutionConfiguration,
    halted: AtomicBool,
}
fn unavailable() -> StorageError {
    StorageError::Unavailable("model agent execution unavailable".into())
}
fn exceeded(usage: Option<ReplyUsage>, budget: &ModelCallBudget) -> bool {
    usage.is_some_and(|u| {
        u.input_tokens > budget.input_token_bound || u.output_tokens > budget.output_token_bound
    })
}
impl ModelAgentExecutor {
    #[must_use]
    pub fn new(
        store: Arc<dyn ModelAgentStore>,
        provider: Arc<dyn ModelAgentProvider>,
        retriever: Arc<dyn ModelRetriever>,
        planning: ModelPlanningConfiguration,
        execution: ModelExecutionConfiguration,
    ) -> Self {
        Self {
            store,
            provider,
            retriever,
            planning,
            execution,
            halted: AtomicBool::new(false),
        }
    }
    #[must_use]
    pub fn is_halted(&self) -> bool {
        self.halted.load(Ordering::SeqCst)
    }
    async fn persist_disable(&self) {
        let first = self
            .store
            .disable_model_planning_configuration(&self.planning.budget.configuration_version)
            .await;
        let second = self
            .store
            .disable_model_execution_configuration(&self.execution.version)
            .await;
        if first.is_err() || second.is_err() {
            tracing::error!("model agent disable persistence failed; executor halted");
        }
    }
    async fn halt(&self) {
        self.halted.store(true, Ordering::SeqCst);
        self.persist_disable().await;
    }
    async fn ready(&self) -> StorageResult<()> {
        if self.is_halted() {
            self.persist_disable().await;
            return Err(unavailable());
        }
        Ok(())
    }
    /// 只有首次精确批准返回 started 才领取；响应丢失后的同键调用只读取结果。
    /// # Errors
    /// 仓储/配置不可用时停止；不重发已领取请求。
    pub async fn approve_planning(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        approval: &AgentQuoteApproval,
    ) -> StorageResult<ModelPlanningRequest> {
        self.ready().await?;
        self.store
            .check_model_planning_configuration(&self.planning)
            .await?;
        let authorization = self
            .store
            .approve_model_planning_request(owner, conversation, request, approval)
            .await?;
        if !authorization.started {
            return Ok(authorization.request);
        }
        let claim = self
            .store
            .claim_model_planning_request(owner, conversation, request)
            .await?;
        let result = async {
            if claim.configuration != self.planning
                || self.is_halted()
                || self
                    .store
                    .get_model_planning_request(owner, conversation, request)
                    .await?
                    .status
                    != "dispatching"
                || self.store.message_revision(owner, conversation).await? != claim.request.revision
            {
                return Err(unavailable());
            }
            let quote = quote_model_planning(
                AgentRequestIdentity {
                    owner: owner.clone(),
                    conversation: ConversationId::new(conversation),
                    request_id: request.into(),
                },
                &claim.snapshot,
                claim.request.revision,
                claim.configuration.budget.clone(),
                claim.configuration.limits,
                QuoteWindow {
                    now_unix_ms: claim.request.created_at_unix_ms,
                    expires_at_unix_ms: claim.request.expires_at_unix_ms,
                },
            )
            .map_err(|_| unavailable())?;
            if quote.quote().digest() != claim.request.digest {
                return Err(unavailable());
            }
            self.store
                .check_model_planning_configuration(&claim.configuration)
                .await?;
            Ok(tokio::time::timeout(
                Duration::from_secs(35),
                self.provider.chat(
                    ModelChatStage::Planning,
                    &quote.context().request,
                    &claim.configuration.budget,
                ),
            )
            .await)
        }
        .await;
        let (outcome, usage) = match result {
            Ok(Ok(Ok(completion))) if !exceeded(completion.usage, &claim.configuration.budget) => (
                ModelPlanningOutcome::Proposed(completion.content.into_bytes()),
                completion.usage,
            ),
            Ok(Ok(Ok(_) | Err(ReplySendError::ContractViolation))) => {
                self.halt().await;
                (ModelPlanningOutcome::Unknown, None)
            }
            Ok(Ok(Err(ReplySendError::InvalidConfiguration | ReplySendError::InvalidResponse))) => {
                (ModelPlanningOutcome::Failed, None)
            }
            _ => (ModelPlanningOutcome::Unknown, None),
        };
        self.store
            .finish_model_planning_request(
                owner,
                conversation,
                request,
                &claim.claim_id,
                outcome,
                usage,
            )
            .await
    }
    /// 独立第二次授权；按冻结查询执行，任何失败/空证据/存储异常均停止后续调用。
    /// # Errors
    /// 完成事务失败不会重发；已领取预算由既有期限收敛。
    pub async fn approve_execution(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        approval: &AgentQuoteApproval,
    ) -> StorageResult<ModelExecutionRequest> {
        self.ready().await?;
        self.store
            .check_model_execution_configuration(&self.execution)
            .await?;
        let authorization = self
            .store
            .approve_model_execution_request(owner, conversation, request, approval)
            .await?;
        if !authorization.started {
            return Ok(authorization.request);
        }
        for ordinal in 0..=authorization.request.calls.tool {
            let claim = self
                .store
                .claim_model_execution_step(owner, conversation, request, ordinal)
                .await?;
            let result = self
                .execute_step(owner, conversation, request, &claim)
                .await;
            let (outcome, usage) = match result {
                Ok(value) => value,
                Err(ReplySendError::ContractViolation) => {
                    self.halt().await;
                    (ModelExecutionOutcome::Unknown, None)
                }
                Err(ReplySendError::InvalidConfiguration | ReplySendError::InvalidResponse) => {
                    (ModelExecutionOutcome::Failed, None)
                }
                Err(ReplySendError::Unknown) => (ModelExecutionOutcome::Unknown, None),
            };
            let saved = self
                .store
                .finish_model_execution_step(
                    owner,
                    conversation,
                    request,
                    &claim.claim_id,
                    outcome,
                    usage,
                )
                .await?;
            if saved.status != "running" {
                return Ok(saved);
            }
        }
        self.store
            .get_model_execution_request(owner, conversation, request)
            .await
    }
    async fn execute_step(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        claim: &personal_ai_storage::model_execution::ModelExecutionClaim,
    ) -> Result<(ModelExecutionOutcome, Option<ReplyUsage>), ReplySendError> {
        let invalid = ReplySendError::InvalidConfiguration;
        if self.is_halted()
            || claim.configuration != self.execution
            || self
                .store
                .get_model_execution_request(owner, conversation, request)
                .await
                .map_err(|_| invalid)?
                .status
                != "running"
            || self
                .store
                .message_revision(owner, conversation)
                .await
                .map_err(|_| invalid)?
                != claim.request.revision
        {
            return Err(invalid);
        }
        self.store
            .check_model_execution_configuration(&self.execution)
            .await
            .map_err(|_| invalid)?;
        if let Some(query) = &claim.query {
            let completion = tokio::time::timeout(
                Duration::from_secs(35),
                self.provider.embed(&query.query, &claim.budget),
            )
            .await
            .map_err(|_| ReplySendError::Unknown)??;
            if exceeded(completion.usage, &claim.budget)
                || completion.embedding.model != claim.budget.model
            {
                return Err(ReplySendError::ContractViolation);
            }
            // 向量化之后再复核取消/到期；不追加任何模型调用。
            if self
                .store
                .get_model_execution_request(owner, conversation, request)
                .await
                .map_err(|_| invalid)?
                .status
                != "running"
            {
                return Err(invalid);
            }
            let hits = tokio::time::timeout(
                Duration::from_secs(10),
                self.retriever
                    .retrieve(owner, &completion.embedding, query.limit),
            )
            .await
            .map_err(|_| ReplySendError::Unknown)?
            .map_err(|_| ReplySendError::InvalidResponse)?;
            Ok((ModelExecutionOutcome::Retrieved(hits), completion.usage))
        } else {
            let plan = plan_model_answer(
                &claim.snapshot,
                claim.request.revision,
                &claim.request.evidence,
            )
            .map_err(|_| invalid)?
            .ok_or(invalid)?;
            let completion = tokio::time::timeout(
                Duration::from_secs(35),
                self.provider
                    .chat(ModelChatStage::Answer, &plan.request, &claim.budget),
            )
            .await
            .map_err(|_| ReplySendError::Unknown)??;
            if exceeded(completion.usage, &claim.budget) {
                return Err(ReplySendError::ContractViolation);
            }
            Ok((
                ModelExecutionOutcome::Answered(completion.content.into_bytes()),
                completion.usage,
            ))
        }
    }
}
