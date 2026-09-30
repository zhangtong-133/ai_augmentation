//! 固定只读计划：显式查询、逐步执行、每计划最多三次，不生成模型工具循环。
use crate::tool_execution::{AuditedToolError, AuditedToolExecutor};
use personal_ai_domain::{ConversationId, UserId};
use personal_ai_storage::agent_plans::{
    AgentPlanStore, MAX_PLAN_TOOL_CALLS, NewAgentPlan, PLAN_VERSION,
};
use personal_ai_tools::{ToolContext, ToolExecutor, ToolRequest};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// 绑定身份、对话、请求 ID、消息版本及全部查询的授权指纹。
#[must_use]
pub fn knowledge_plan_digest(owner: &UserId, conversation: &str, input: &NewAgentPlan) -> String {
    let definition = json!({
        "version": PLAN_VERSION,
        "owner": owner.as_str(),
        "conversation": conversation,
        "request_id": input.request_id,
        "revision": input.expected_revision,
        "tool": "knowledge_search",
        "call_limit": input.searches.len(),
        "searches": input.searches,
    });
    format!("{:x}", Sha256::digest(definition.to_string().as_bytes()))
}

pub struct KnowledgePlanExecutor {
    store: Arc<dyn AgentPlanStore>,
    tools: AuditedToolExecutor,
}
impl KnowledgePlanExecutor {
    #[must_use]
    pub fn new(store: Arc<dyn AgentPlanStore>, executor: Arc<ToolExecutor>) -> Self {
        Self {
            tools: AuditedToolExecutor::new(store.clone(), executor),
            store,
        }
    }

    /// 只由首次显式授权触发。中断、领取/存储失败后停止，不扫描或重试旧计划。
    pub async fn run(&self, owner: &UserId, conversation: &str, request: &str) {
        let context = ToolContext {
            user_id: owner.clone(),
            conversation_id: ConversationId::new(conversation),
        };
        for _ in 0..MAX_PLAN_TOOL_CALLS {
            let claim = match self
                .store
                .claim_agent_step(owner, conversation, request)
                .await
            {
                Ok(Some(claim)) => claim,
                Ok(None) => return,
                Err(_) => {
                    tracing::warn!("agent plan claim unavailable");
                    return;
                }
            };
            let arguments = ToolRequest {
                arguments_json: json!(claim.arguments).to_string(),
            };
            let result = self
                .tools
                .execute_registered(&claim.call_id, "knowledge_search", &context, &arguments)
                .await;
            let output = match result {
                Ok(result) => {
                    if let Ok(value) = serde_json::from_str(&result.response.content) {
                        Some(value)
                    } else {
                        // 当前固定工具只返回 JSON；无法解释的成功结果不继续执行。
                        tracing::warn!("agent tool output invalid");
                        return;
                    }
                }
                Err(AuditedToolError::Execution(_)) => None,
                Err(_) => {
                    tracing::warn!("agent tool audit unavailable");
                    return;
                }
            };
            let failed = output.is_none();
            if self
                .store
                .finish_agent_step(owner, conversation, request, &claim.call_id, output)
                .await
                .is_err()
            {
                tracing::warn!("agent plan completion unavailable");
                return;
            }
            if failed {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_ai_storage::agent_plans::KnowledgeQuery;

    #[test]
    fn plan_validation_caps_calls_and_rejects_blank_duplicate_or_invalid_searches() {
        let query = KnowledgeQuery {
            query: "资料".into(),
            limit: 5,
        };
        let mut input = NewAgentPlan {
            request_id: "plan".into(),
            expected_revision: 1,
            searches: vec![query],
        };
        assert!(input.validate().is_ok());
        input.searches.push(KnowledgeQuery {
            query: " 资料 ".into(),
            limit: 1,
        });
        assert!(input.validate().is_err());
        input.searches = (0..4)
            .map(|n| KnowledgeQuery {
                query: n.to_string(),
                limit: 5,
            })
            .collect();
        assert!(input.validate().is_err());
        for (text, limit) in [
            (" ".to_owned(), 5),
            ("a\0b".to_owned(), 5),
            ("字".repeat(1001), 5),
            ("ok".to_owned(), 6),
        ] {
            input.searches = vec![KnowledgeQuery { query: text, limit }];
            assert!(input.validate().is_err());
        }
    }

    #[test]
    fn consent_digest_changes_with_identity_version_query_limit_and_order() {
        let owner = UserId::new("owner");
        let mut input = NewAgentPlan {
            request_id: "plan".into(),
            expected_revision: 1,
            searches: vec![
                KnowledgeQuery {
                    query: "first".into(),
                    limit: 5,
                },
                KnowledgeQuery {
                    query: "second".into(),
                    limit: 5,
                },
            ],
        };
        let digest = knowledge_plan_digest(&owner, "conversation", &input);
        assert_eq!(
            digest,
            knowledge_plan_digest(&owner, "conversation", &input)
        );
        assert_ne!(
            digest,
            knowledge_plan_digest(&UserId::new("other"), "conversation", &input)
        );
        assert_ne!(digest, knowledge_plan_digest(&owner, "other", &input));
        input.searches.reverse();
        assert_ne!(
            digest,
            knowledge_plan_digest(&owner, "conversation", &input)
        );
        input.searches.reverse();
        input.expected_revision = 2;
        assert_ne!(
            digest,
            knowledge_plan_digest(&owner, "conversation", &input)
        );
        input.expected_revision = 1;
        input.searches[0].limit = 1;
        assert_ne!(
            digest,
            knowledge_plan_digest(&owner, "conversation", &input)
        );
    }
}
