//! RSS 单次执行器：先领取，再调用一次传输，最后写回；没有自动重试或后台轮询。
use personal_ai_domain::UserId;
use personal_ai_feeds::transport::{FeedTransport, FetchError};
use personal_ai_storage::{
    StorageError,
    feeds::{Collection, CollectionFailure, CollectionOutcome, FeedStore},
};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

// 进程共享，重新构造执行器不能增加并发名额。
static SLOTS: Semaphore = Semaphore::const_new(2);
const STORAGE_TIMEOUT: Duration = Duration::from_secs(5);
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, PartialEq, Eq)]
pub enum FeedExecutionError {
    Busy,
    Storage(StorageError),
    /// 领取/写回事务的提交结果不明确，调用方只能查询原请求，不得自动重新执行。
    OutcomeUnknown,
}

pub struct FeedExecutor {
    store: Arc<dyn FeedStore>,
    transport: Arc<dyn FeedTransport>,
}
impl FeedExecutor {
    #[must_use]
    pub fn new(store: Arc<dyn FeedStore>, transport: Arc<dyn FeedTransport>) -> Self {
        Self { store, transport }
    }

    /// 只能由会话/CSRF 校验后的明确确认入口调用；不接受客户端提供的完整计划。
    /// 调用方断开后已启动的任务继续持有名额，并在有限时间内尝试保存结果。
    /// # Errors
    /// 名额不足时不领取；其他错误不重试，提交结果未知时查询原请求或显式恢复。
    pub async fn execute(
        &self,
        owner: &UserId,
        request: &str,
        digest: &str,
    ) -> Result<Collection, FeedExecutionError> {
        let permit = SLOTS.try_acquire().map_err(|_| FeedExecutionError::Busy)?;
        let (store, transport, owner, request, digest) = (
            self.store.clone(),
            self.transport.clone(),
            owner.clone(),
            request.to_owned(),
            digest.to_owned(),
        );
        tokio::spawn(async move {
            let _permit = permit;
            let claim = tokio::time::timeout(
                STORAGE_TIMEOUT,
                store.claim_collection(&owner, &request, &digest),
            )
            .await
            .map_err(|_| FeedExecutionError::OutcomeUnknown)?
            .map_err(FeedExecutionError::Storage)?;
            fetch_and_finish(store, transport, claim).await
        })
        .await
        .map_err(|_| FeedExecutionError::OutcomeUnknown)?
    }
}

async fn fetch_and_finish(
    store: Arc<dyn FeedStore>,
    transport: Arc<dyn FeedTransport>,
    claim: personal_ai_storage::feeds::CollectionClaim,
) -> Result<Collection, FeedExecutionError> {
    let outcome =
        match tokio::time::timeout(FETCH_TIMEOUT, transport.fetch(&claim.plan.source_url)).await {
            Ok(Ok(bytes)) => CollectionOutcome::Response(bytes),
            Err(_) | Ok(Err(FetchError::Timeout | FetchError::Unavailable)) => {
                CollectionOutcome::Failure(CollectionFailure::Unknown)
            }
            Ok(Err(_)) => CollectionOutcome::Failure(CollectionFailure::Transport),
        };
    tokio::time::timeout(STORAGE_TIMEOUT, store.finish_collection(&claim, outcome))
        .await
        .map_err(|_| FeedExecutionError::OutcomeUnknown)?
        .map_err(FeedExecutionError::Storage)
}

/// 内部周期执行入口；应用启动尚未接入自动轮询。
pub struct ScheduledFeedExecutor {
    store: Arc<dyn FeedStore>,
    schedules: Arc<dyn personal_ai_storage::feed_schedules::FeedScheduleExecutionStore>,
    transport: Arc<dyn FeedTransport>,
}
impl ScheduledFeedExecutor {
    #[must_use]
    pub fn new<T>(store: Arc<T>, transport: Arc<dyn FeedTransport>) -> Self
    where
        T: FeedStore + personal_ai_storage::feed_schedules::FeedScheduleExecutionStore + 'static,
    {
        Self {
            store: store.clone(),
            schedules: store,
            transport,
        }
    }
    /// 仅处理当前时段，先原子领取，再复核发送栅栏；不重试。
    /// # Errors
    /// 额度/授权变化、存储提交未知或传输故障时保留一次性账本。
    pub async fn execute(
        &self,
        owner: &UserId,
        schedule: &str,
    ) -> Result<Collection, FeedExecutionError> {
        let permit = SLOTS.try_acquire().map_err(|_| FeedExecutionError::Busy)?;
        let (store, schedules, transport, owner, schedule) = (
            self.store.clone(),
            self.schedules.clone(),
            self.transport.clone(),
            owner.clone(),
            schedule.to_owned(),
        );
        tokio::spawn(async move {
            let _permit = permit;
            let claim = tokio::time::timeout(
                STORAGE_TIMEOUT,
                schedules.claim_scheduled_collection(&owner, &schedule),
            )
            .await
            .map_err(|_| FeedExecutionError::OutcomeUnknown)?
            .map_err(FeedExecutionError::Storage)?;
            let allowed = tokio::time::timeout(
                STORAGE_TIMEOUT,
                schedules.dispatch_scheduled_collection(&claim),
            )
            .await
            .map_err(|_| FeedExecutionError::OutcomeUnknown)?
            .map_err(FeedExecutionError::Storage)?;
            if !allowed {
                return tokio::time::timeout(
                    STORAGE_TIMEOUT,
                    store.finish_collection(
                        &claim,
                        CollectionOutcome::Failure(CollectionFailure::Unknown),
                    ),
                )
                .await
                .map_err(|_| FeedExecutionError::OutcomeUnknown)?
                .map_err(FeedExecutionError::Storage);
            }
            fetch_and_finish(store, transport, claim).await
        })
        .await
        .map_err(|_| FeedExecutionError::OutcomeUnknown)?
    }
}
