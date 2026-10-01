//! 模型建议的第二阶段内部预算协议；不执行检索或模型发送。
use crate::{
    BoxFuture, StorageResult,
    agent_plans::KnowledgeQuery,
    messages::MessageSnapshot,
    model_agents::{AgentBudgetLimits, AgentCallCounts, AgentQuoteApproval, ModelCallBudget},
    reply_budgets::ReplyUsage,
};
use personal_ai_domain::UserId;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelExecutionConfiguration {
    pub version: String,
    pub embedding: ModelCallBudget,
    pub answer: ModelCallBudget,
    pub limits: AgentBudgetLimits,
}
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModelExecutionRequest {
    /// 与成功的第一阶段规划请求相同；每个规划只允许一个第二阶段报价。
    pub request_id: String,
    pub conversation_id: String,
    pub revision: i64,
    pub digest: String,
    pub status: String,
    pub currency: String,
    pub amount: i64,
    pub calls: AgentCallCounts,
    pub searches: Option<Vec<KnowledgeQuery>>,
    pub created_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
}
pub struct ModelExecutionAuthorization {
    pub request: ModelExecutionRequest,
    pub started: bool,
}
/// COMMIT 确认后返回的内部凭据。不能序列化给用户，也不能跨调用重复使用。
pub struct ModelExecutionClaim {
    pub request: ModelExecutionRequest,
    pub ordinal: u32,
    pub claim_id: String,
    pub call_id: String,
    pub query: Option<KnowledgeQuery>,
    pub snapshot: MessageSnapshot,
    pub budget: ModelCallBudget,
}
#[derive(Clone, Copy)]
pub enum ModelExecutionOutcome {
    /// 由未来执行器确认成功；检索输出字节数仅用于审计，不保存正文。
    Succeeded {
        output_bytes: i32,
    },
    Failed,
    Unknown,
}
pub trait ModelExecutionStore: Send + Sync {
    fn register_model_execution_configuration(
        &self,
        configuration: &ModelExecutionConfiguration,
    ) -> BoxFuture<'_, StorageResult<()>>;
    fn disable_model_execution_configuration(
        &self,
        version: &str,
    ) -> BoxFuture<'_, StorageResult<()>>;
    fn check_model_execution_configuration(
        &self,
        configuration: &ModelExecutionConfiguration,
    ) -> BoxFuture<'_, StorageResult<()>>;
    fn create_model_execution_request(
        &self,
        owner: &UserId,
        conversation: &str,
        planning_request: &str,
        configuration_version: &str,
    ) -> BoxFuture<'_, StorageResult<ModelExecutionRequest>>;
    fn get_model_execution_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ModelExecutionRequest>>;
    fn approve_model_execution_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        approval: &AgentQuoteApproval,
    ) -> BoxFuture<'_, StorageResult<ModelExecutionAuthorization>>;
    fn claim_model_execution_step(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        ordinal: u32,
    ) -> BoxFuture<'_, StorageResult<ModelExecutionClaim>>;
    fn finish_model_execution_step(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        claim_id: &str,
        outcome: ModelExecutionOutcome,
        usage: Option<ReplyUsage>,
    ) -> BoxFuture<'_, StorageResult<ModelExecutionRequest>>;
    fn cancel_model_execution_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ModelExecutionRequest>>;
}
