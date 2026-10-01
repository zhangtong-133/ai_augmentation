//! 内部模型规划仓储协议；价格由部署端登记，不接收用户 HTTP 价格或模型授权。
use crate::{
    BoxFuture, StorageResult, agent_plans::KnowledgeQuery, messages::MessageSnapshot,
    reply_budgets::ReplyUsage,
};
use personal_ai_domain::UserId;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCallBudget {
    pub configuration_version: String,
    pub provider: String,
    pub model: String,
    pub price_version: String,
    pub counter_version: String,
    pub currency: String,
    pub input_price_per_million: u64,
    pub output_price_per_million: u64,
    pub input_token_bound: u64,
    pub output_token_bound: u64,
    pub valid_until_unix_ms: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCallCounts {
    pub chat: u32,
    pub embedding: u32,
    pub tool: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentBudgetLimits {
    pub phase_amount: i64,
    pub daily_amount: i64,
    pub daily_model_calls: u32,
    pub daily_tool_calls: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentQuoteApproval {
    pub digest: String,
    pub accepted_currency: String,
    pub accepted_amount: i64,
    pub accepted_calls: AgentCallCounts,
    pub acknowledge_cost: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPlanningConfiguration {
    pub budget: ModelCallBudget,
    pub limits: AgentBudgetLimits,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewModelPlanningRequest {
    pub request_id: String,
    pub expected_revision: i64,
    pub configuration_version: String,
}

/// 可供页面查询的记录；不会序列化冻结上下文、内部预算或领取凭据。
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct ModelPlanningRequest {
    pub request_id: String,
    pub conversation_id: String,
    pub revision: i64,
    pub version: String,
    pub digest: String,
    pub status: String,
    pub currency: String,
    pub amount: i64,
    pub calls: AgentCallCounts,
    pub searches: Option<Vec<KnowledgeQuery>>,
    pub created_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub approved_at_unix_ms: Option<i64>,
}

pub struct ModelPlanningAuthorization {
    pub request: ModelPlanningRequest,
    /// 首次批准时为 true；不代表已经领取或允许重复派发。
    pub started: bool,
}

/// 只有领取事务提交确认后返回；不能序列化到用户响应。
pub struct ModelPlanningClaim {
    pub request: ModelPlanningRequest,
    pub claim_id: String,
    pub snapshot: MessageSnapshot,
    pub configuration: ModelPlanningConfiguration,
}

pub enum ModelPlanningOutcome {
    /// 供应商返回的纯 JSON，仓储再次执行严格建议解码。
    Proposed(Vec<u8>),
    Failed,
    Unknown,
}

pub trait ModelPlanningStore: Send + Sync {
    /// 授权前确认请求属于当前部署版本，配置切换不能先扣款再拒绝发送。
    fn check_model_planning_request_configuration(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        version: &str,
    ) -> BoxFuture<'_, StorageResult<()>>;

    fn register_model_planning_configuration(
        &self,
        configuration: &ModelPlanningConfiguration,
    ) -> BoxFuture<'_, StorageResult<()>>;
    fn disable_model_planning_configuration(
        &self,
        version: &str,
    ) -> BoxFuture<'_, StorageResult<()>>;
    fn create_model_planning_request(
        &self,
        owner: &UserId,
        conversation: &str,
        input: &NewModelPlanningRequest,
    ) -> BoxFuture<'_, StorageResult<ModelPlanningRequest>>;
    fn get_model_planning_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ModelPlanningRequest>>;
    fn list_model_planning_requests(
        &self,
        owner: &UserId,
        conversation: &str,
    ) -> BoxFuture<'_, StorageResult<Vec<ModelPlanningRequest>>>;
    fn approve_model_planning_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        approval: &AgentQuoteApproval,
    ) -> BoxFuture<'_, StorageResult<ModelPlanningAuthorization>>;
    fn claim_model_planning_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ModelPlanningClaim>>;
    /// 发送前再次检查持久化配置，不能代替领取事务，也不能撤回已经发出的请求。
    fn check_model_planning_configuration(
        &self,
        configuration: &ModelPlanningConfiguration,
    ) -> BoxFuture<'_, StorageResult<()>>;
    fn finish_model_planning_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        claim_id: &str,
        outcome: ModelPlanningOutcome,
        usage: Option<ReplyUsage>,
    ) -> BoxFuture<'_, StorageResult<ModelPlanningRequest>>;
    fn cancel_model_planning_request(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ModelPlanningRequest>>;
}
