use crate::{Tool, ToolContext, ToolError, ToolRequest, ToolResponse};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

#[derive(Debug, Eq, PartialEq)]
pub enum ExecutionError {
    UnknownTool,
    InvalidArguments,
    Busy,
    Timeout,
    OutputTooLarge,
    Tool(ToolError),
}

/// 仅执行应用显式注册的工具；不动态加载程序或自动重试。
pub struct ToolExecutor {
    tools: BTreeMap<&'static str, Arc<dyn Tool>>,
    slots: Semaphore,
}

impl ToolExecutor {
    /// 创建固定白名单，拒绝重复工具名。
    ///
    /// # Errors
    /// 重复名称返回错误，不覆盖已有工具。
    pub fn new(tools: Vec<Arc<dyn Tool>>) -> Result<Self, &'static str> {
        let mut registry = BTreeMap::new();
        for tool in tools {
            if registry.insert(tool.name(), tool).is_some() {
                return Err("duplicate tool name");
            }
        }
        Ok(Self {
            tools: registry,
            slots: Semaphore::new(2),
        })
    }

    pub fn tools(&self) -> impl Iterator<Item = &Arc<dyn Tool>> {
        self.tools.values()
    }

    /// 上下文必须来自已认证的应用层；工具仍负责所有者隔离及具体参数校验。
    ///
    /// # Errors
    /// 未注册、参数过大/非对象、并发耗尽、超时或工具失败时返回错误。
    /// 超时只取消当前 future，不能撤销工具已经发出的外部请求。
    pub async fn execute(
        &self,
        name: &str,
        context: &ToolContext,
        request: &ToolRequest,
    ) -> Result<ToolResponse, ExecutionError> {
        let tool = self.tools.get(name).ok_or(ExecutionError::UnknownTool)?;
        if request.arguments_json.len() > 8192
            || !serde_json::from_str::<serde_json::Value>(&request.arguments_json)
                .is_ok_and(|value| value.is_object())
        {
            return Err(ExecutionError::InvalidArguments);
        }
        let _permit = self.slots.try_acquire().map_err(|_| ExecutionError::Busy)?;
        let response =
            tokio::time::timeout(Duration::from_secs(35), tool.execute(context, request))
                .await
                .map_err(|_| ExecutionError::Timeout)?
                .map_err(ExecutionError::Tool)?;
        if response.content.len() > 65536 {
            return Err(ExecutionError::OutputTooLarge);
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BoxFuture;
    use personal_ai_domain::{ConversationId, UserId};

    struct Echo;
    impl Tool for Echo {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn description(&self) -> &'static str {
            "测试工具"
        }
        fn input_schema_json(&self) -> &'static str {
            "{}"
        }
        fn execute(
            &self,
            context: &ToolContext,
            _: &ToolRequest,
        ) -> BoxFuture<'_, Result<ToolResponse, ToolError>> {
            let owner = context.user_id.to_string();
            Box::pin(async move {
                Ok(ToolResponse {
                    content: owner,
                    is_error: false,
                })
            })
        }
    }

    #[tokio::test]
    async fn whitelist_arguments_owner_and_concurrency_are_enforced() {
        let executor = ToolExecutor::new(vec![Arc::new(Echo)]).unwrap();
        assert!(ToolExecutor::new(vec![Arc::new(Echo), Arc::new(Echo)]).is_err());
        let context = ToolContext {
            user_id: UserId::new("owner"),
            conversation_id: ConversationId::new("request"),
        };
        let request = ToolRequest {
            arguments_json: "{}".into(),
        };
        assert_eq!(
            executor.execute("missing", &context, &request).await,
            Err(ExecutionError::UnknownTool)
        );
        for input in ["[]".to_string(), "bad".into(), " ".repeat(8193)] {
            assert_eq!(
                executor
                    .execute(
                        "echo",
                        &context,
                        &ToolRequest {
                            arguments_json: input
                        }
                    )
                    .await,
                Err(ExecutionError::InvalidArguments)
            );
        }
        let permits = executor.slots.acquire_many(2).await.unwrap();
        assert_eq!(
            executor.execute("echo", &context, &request).await,
            Err(ExecutionError::Busy)
        );
        drop(permits);
        assert_eq!(
            executor
                .execute("echo", &context, &request)
                .await
                .unwrap()
                .content,
            "owner"
        );
    }

    struct Hanging;
    impl Tool for Hanging {
        fn name(&self) -> &'static str {
            "hanging"
        }
        fn description(&self) -> &'static str {
            "超时测试"
        }
        fn input_schema_json(&self) -> &'static str {
            "{}"
        }
        fn execute(
            &self,
            _: &ToolContext,
            _: &ToolRequest,
        ) -> BoxFuture<'_, Result<ToolResponse, ToolError>> {
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_and_output_limit_release_capacity() {
        let executor = ToolExecutor::new(vec![Arc::new(Hanging), Arc::new(Echo)]).unwrap();
        let context = ToolContext {
            user_id: UserId::new("x".repeat(65537)),
            conversation_id: ConversationId::new("request"),
        };
        let request = ToolRequest {
            arguments_json: "{}".into(),
        };
        assert_eq!(
            executor.execute("hanging", &context, &request).await,
            Err(ExecutionError::Timeout)
        );
        assert_eq!(executor.slots.available_permits(), 2);
        assert_eq!(
            executor.execute("echo", &context, &request).await,
            Err(ExecutionError::OutputTooLarge)
        );
        assert_eq!(executor.slots.available_permits(), 2);
    }
}
