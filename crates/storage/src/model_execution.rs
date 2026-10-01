//! 模型建议的第二阶段内部预算、证据与回答协议；不执行模型发送。
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
    #[serde(default)]
    pub evidence: Vec<ModelEvidence>,
    #[serde(default)]
    pub answer: Option<ModelAnswer>,
    pub created_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
}
pub struct ModelExecutionAuthorization {
    pub request: ModelExecutionRequest,
    pub started: bool,
}
/// COMMIT 确认后返回的内部凭据。不能序列化给用户，也不能跨调用重复使用。
pub struct ModelExecutionClaim {
    pub configuration: ModelExecutionConfiguration,
    pub request: ModelExecutionRequest,
    pub ordinal: u32,
    pub claim_id: String,
    pub call_id: String,
    pub query: Option<KnowledgeQuery>,
    pub snapshot: MessageSnapshot,
    pub budget: ModelCallBudget,
}
/// 执行器提交召回片段，仓储再次校验所有者及权威文本；不信任向量载荷。
#[derive(Clone, Debug)]
pub struct RetrievedChunk {
    pub document_id: String,
    pub ordinal: usize,
    pub text: String,
}
/// 按首次出现顺序分配稳定引用 ID，跨查询去重。
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEvidence {
    pub id: usize,
    pub document_id: String,
    pub ordinal: usize,
    pub title: String,
    pub source: String,
    pub text: String,
}
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelAnswer {
    pub insufficient_evidence: bool,
    pub answer: String,
    pub citations: Vec<usize>,
}
#[derive(Clone)]
pub enum ModelExecutionOutcome {
    /// 每次最多为授权 limit 条；与步骤结算原子保存。
    Retrieved(Vec<RetrievedChunk>),
    /// 原始严格 JSON；只有当前回答步骤可以提交。
    Answered(Vec<u8>),
    Failed,
    Unknown,
}
pub trait ModelExecutionStore: Send + Sync {
    /// 授权前确认请求属于当前部署版本，配置切换不能先扣款再拒绝发送。
    fn check_model_execution_request_configuration(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        version: &str,
    ) -> BoxFuture<'_, StorageResult<()>>;

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
