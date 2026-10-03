#![forbid(unsafe_code)]

pub mod budget;
pub mod feed_value;
pub mod feed_value_execution;
pub mod feeds;
pub mod knowledge_plan;
pub mod model_answer;
pub mod model_executor;
pub mod model_plan;
pub mod reply;
pub mod reply_executor;
pub mod tool_execution;

pub mod schedules;

use std::future::Future;
use std::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
