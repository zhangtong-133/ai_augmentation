//! 用户可审阅的只读检索计划与持久化授权；不接收模型提供的权限。
use crate::{BoxFuture, StorageError, StorageResult, tool_calls::ToolCallStore};
use personal_ai_domain::UserId;

pub const MAX_PLAN_TOOL_CALLS: usize = 3;
pub const PLAN_VERSION: &str = "knowledge-search-v1";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeQuery {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}
fn default_limit() -> usize {
    5
}

impl KnowledgeQuery {
    /// # Errors
    /// 空查询、NUL、超过 1000 字符或结果数量超出 1–5 时拒绝。
    pub fn validate(&self) -> StorageResult<()> {
        if self.query.trim().is_empty()
            || self.query.contains('\0')
            || self.query.chars().count() > 1000
            || !(1..=5).contains(&self.limit)
        {
            return Err(StorageError::InvalidData("invalid knowledge query".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAgentPlan {
    pub request_id: String,
    pub expected_revision: i64,
    pub searches: Vec<KnowledgeQuery>,
}

impl NewAgentPlan {
    /// # Errors
    /// 每计划必须含 1–3 个互不重复的有效查询，且引用一个已有消息版本。
    pub fn validate(&self) -> StorageResult<()> {
        if !(1..=100).contains(&self.expected_revision)
            || self.searches.is_empty()
            || self.searches.len() > MAX_PLAN_TOOL_CALLS
        {
            return Err(StorageError::InvalidData("invalid agent plan".into()));
        }
        for (index, search) in self.searches.iter().enumerate() {
            search.validate()?;
            if self.searches[..index]
                .iter()
                .any(|previous| previous.query.trim() == search.query.trim())
            {
                return Err(StorageError::InvalidData("duplicate plan search".into()));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AgentPlanStep {
    pub ordinal: i32,
    pub call_id: String,
    pub tool: String,
    pub arguments: KnowledgeQuery,
    pub status: String,
    pub output: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AgentPlan {
    pub request_id: String,
    pub conversation_id: String,
    pub revision: i64,
    pub version: String,
    pub digest: String,
    pub status: String,
    pub tool_call_limit: i32,
    pub attempted: i32,
    pub steps: Vec<AgentPlanStep>,
    pub created_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub approved_at_unix_ms: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct AgentPlanApproval {
    pub plan_digest: String,
    pub accepted_call_limit: i32,
    pub acknowledge_embedding_cost: bool,
}

pub struct AgentPlanAuthorization {
    pub plan: AgentPlan,
    /// 只有本次事务首次授权时为 true；重放不能启动另一个执行器。
    pub started: bool,
}

pub struct AgentStepClaim {
    pub call_id: String,
    pub arguments: KnowledgeQuery,
}

pub trait AgentPlanStore: ToolCallStore {
    fn create_agent_plan(
        &self,
        owner: &UserId,
        conversation: &str,
        input: &NewAgentPlan,
    ) -> BoxFuture<'_, StorageResult<AgentPlan>>;
    fn get_agent_plan(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<AgentPlan>>;
    fn list_agent_plans(
        &self,
        owner: &UserId,
        conversation: &str,
    ) -> BoxFuture<'_, StorageResult<Vec<AgentPlan>>>;
    fn approve_agent_plan(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        approval: &AgentPlanApproval,
    ) -> BoxFuture<'_, StorageResult<AgentPlanAuthorization>>;
    fn cancel_agent_plan(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<AgentPlan>>;
    /// 必须原子检查授权、对话版本、计划上限，并登记工具日预算/审计后才返回。
    fn claim_agent_step(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Option<AgentStepClaim>>>;
    /// 工具审计已写入终态后，保存成功结果；失败即终止其余步骤。
    fn finish_agent_step(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        call: &str,
        output: Option<serde_json::Value>,
    ) -> BoxFuture<'_, StorageResult<()>>;
}
