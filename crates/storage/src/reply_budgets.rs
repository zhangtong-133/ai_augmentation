//! 内部货币预算端口；不直接接受 HTTP 客户端提供的价格或计数。
use crate::{
    BoxFuture, StorageResult,
    replies::{Reply, ReplyConfiguration, ReplyContext, ReplyOutcome, ReplyStore},
};
use personal_ai_domain::UserId;
use std::sync::Arc;

/// 单次请求的服务端预算快照，金额单位为币种的百万分之一。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReplyBudget {
    pub currency: String,
    pub provider: String,
    pub price_version: String,
    pub counter_version: String,
    pub input_price_per_million: u64,
    pub output_price_per_million: u64,
    pub input_token_bound: u64,
    pub output_token_bound: u64,
    pub request_limit: i64,
    pub daily_limit: i64,
}

/// 对事务内冻结的完整上下文同步计数。实现必须是有界纯计算，不能访问网络。
/// 必须核验固定模型、封装开销和全部计费输出的硬上限；无法证明时返回错误。
pub trait ReplyBudgetPlanner: Send + Sync {
    /// # Errors
    /// 无法证明完整请求的费用上界或配置无效时拒绝规划。
    fn plan(&self, context: &ReplyContext) -> StorageResult<ReplyBudget>;
}

/// 仅可传入经过供应商适配器核验的完整计费用量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplyUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub trait BudgetedReplyStore: ReplyStore {
    /// 同事务预留次数和金额。已有请求重放不会调用 planner 或重新计费。
    /// 包括旧的无金额请求：不会补扣款，本接口返回值不能单独作为付费发送授权。
    fn reserve_budgeted_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        revision: i64,
        configuration: &ReplyConfiguration,
        planner: Arc<dyn ReplyBudgetPlanner>,
    ) -> BoxFuture<'_, StorageResult<Reply>>;

    /// 仅及时成功的首次完成可凭可信 usage 退差额；其他结果全额保留。
    fn finish_budgeted_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        outcome: ReplyOutcome,
        usage: Option<ReplyUsage>,
    ) -> BoxFuture<'_, StorageResult<Reply>>;
}
