//! 内部单次传输端口；实现不得自行重试、访问条目链接或批准采集。
use std::{future::Future, pin::Pin};

pub type FetchFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 固定错误类型，禁止在错误中泄露 URL 查询参数或响应正文。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchError {
    InvalidSource,
    Blocked,
    Busy,
    Redirect,
    Unsupported,
    TooLarge,
    Timeout,
    Unavailable,
}

pub trait FeedTransport: Send + Sync {
    /// 调用方必须先领取持久化授权；只获取此来源，最多 8 秒、1 MiB，不解析条目。
    fn fetch(&self, source: &str) -> FetchFuture<'_, Result<Vec<u8>, FetchError>>;
}
