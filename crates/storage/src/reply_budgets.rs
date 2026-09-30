//! 内部货币预算端口；不直接接受 HTTP 客户端提供的价格或计数。
use crate::{
    BoxFuture, StorageResult,
    replies::{PendingReply, Reply, ReplyConfiguration, ReplyContext, ReplyOutcome, ReplyStore},
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

/// 已提交的一次性领取结果。仅内部执行器使用，不得序列化给客户端。
/// 领取后仍须在发送前复核配置；取消/停用无法撤回已发出的网络请求。
pub struct BudgetedReplyClaim {
    pub reply: Reply,
    pub budget: ReplyBudget,
    pub reserved: i64,
}

/// 部署端内部端口。配置版本不可变，停用不可逆，禁止接入用户 HTTP 参数。
/// 用户可查看的账本记录；无正文、密钥、内部计数器或价格明细。
pub struct ReplyMoneyReceipt {
    pub request_id: String,
    pub currency: String,
    pub reserved: i64,
    pub charged: Option<i64>,
    pub settlement: Option<String>,
}

pub trait ReplyDispatchStore: BudgetedReplyStore {
    /// 新请求必须在预留事务内检查已登记配置；同键重放不重新报价。
    fn reserve_active_budgeted_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
        revision: i64,
        configuration: &ReplyConfiguration,
        planner: Arc<dyn ReplyBudgetPlanner>,
    ) -> BoxFuture<'_, StorageResult<Reply>>;

    /// 验证对话归属及未删除状态后返回其金额记录；模式停用后仍可查询。
    fn reply_money_receipts(
        &self,
        owner: &UserId,
        conversation: &str,
    ) -> BoxFuture<'_, StorageResult<Vec<ReplyMoneyReceipt>>>;

    /// 仅扫描指定配置的金额请求，避免旧夹具或其他配置队列阻塞执行器。
    fn pending_budgeted_replies(
        &self,
        configuration: &ReplyConfiguration,
    ) -> BoxFuture<'_, StorageResult<Vec<PendingReply>>>;

    /// 注册显式审核过的价格配置及有效期（Unix 毫秒）。同版本只允许完全相同重放。
    fn register_reply_configuration(
        &self,
        configuration: &ReplyConfiguration,
        budget: &ReplyBudget,
        valid_until_unix_ms: i64,
    ) -> BoxFuture<'_, StorageResult<()>>;

    /// 持久化停用，重复调用安全；不得通过重新注册恢复。
    fn disable_reply_configuration(&self, revision: &str) -> BoxFuture<'_, StorageResult<()>>;

    /// 检查数据库时钟下的有效性；供发送前复核，不能替代一次性领取。
    fn check_reply_configuration(
        &self,
        configuration: &ReplyConfiguration,
        budget: &ReplyBudget,
    ) -> BoxFuture<'_, StorageResult<()>>;

    /// 原子核验未结算预算及有效配置，并只领取一次；提交确认后才返回凭据。
    /// 无金额的历史请求、已取消或已领取请求均拒绝。
    fn claim_budgeted_reply(
        &self,
        owner: &UserId,
        conversation: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<BudgetedReplyClaim>>;
}
