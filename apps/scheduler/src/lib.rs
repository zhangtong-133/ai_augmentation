#![forbid(unsafe_code)]
//! 周期 RSS 的有界批次运行器；只有应用显式启用后才构造。
use personal_ai_agent_core::feeds::ScheduledFeedExecutor;
use personal_ai_feeds::transport::FeedTransport;
use personal_ai_storage::{
    StorageError,
    feed_schedules::{FeedScheduleCursor, FeedScheduleExecutionStore, FeedScheduleScanStore},
    feeds::FeedStore,
};
use std::{sync::Arc, time::Duration};

#[derive(Default, Debug)]
pub struct FeedBatchResult {
    pub recovered: u32,
    pub completed: u32,
    pub skipped_or_unknown: u32,
}
pub struct FeedScheduleRunner<T> {
    store: Arc<T>,
    executor: ScheduledFeedExecutor,
    due_cursor: Option<FeedScheduleCursor>,
    recovery_cursor: Option<FeedScheduleCursor>,
}
impl<T> FeedScheduleRunner<T>
where
    T: FeedStore + FeedScheduleExecutionStore + FeedScheduleScanStore + 'static,
{
    #[must_use]
    pub fn new(store: Arc<T>, transport: Arc<dyn FeedTransport>) -> Self {
        Self {
            executor: ScheduledFeedExecutor::new(store.clone(), transport),
            store,
            due_cursor: None,
            recovery_cursor: None,
        }
    }
    /// 扫描最多 20 个过期请求和 20 个到期授权；单条失败不会阻止后续游标推进。
    /// # Errors
    /// 扫描失败或超时返回脱敏错误；调用方不得重发已领取请求。
    pub async fn tick(&mut self) -> Result<FeedBatchResult, StorageError> {
        let mut result = FeedBatchResult::default();
        let expired = tokio::time::timeout(
            Duration::from_secs(5),
            self.store
                .scan_expired_scheduled_collections(self.recovery_cursor.as_ref()),
        )
        .await
        .map_err(|_| StorageError::Unavailable("feed recovery scan timeout".into()))??;
        self.recovery_cursor = expired.next_cursor;
        for item in expired.items {
            match tokio::time::timeout(
                Duration::from_secs(5),
                self.store.recover_collection(&item.owner, &item.id),
            )
            .await
            {
                Ok(Ok(_)) => result.recovered += 1,
                _ => result.skipped_or_unknown += 1,
            }
        }
        let due = tokio::time::timeout(
            Duration::from_secs(5),
            self.store.scan_due_feed_schedules(self.due_cursor.as_ref()),
        )
        .await
        .map_err(|_| StorageError::Unavailable("feed due scan timeout".into()))??;
        self.due_cursor = due.next_cursor;
        for item in due.items {
            match self.executor.execute(&item.owner, &item.id).await {
                Ok(_) => result.completed += 1,
                Err(_) => result.skipped_or_unknown += 1,
            }
        }
        Ok(result)
    }
}
