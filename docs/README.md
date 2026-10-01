# 文档索引

当前能力与启动方式见 [项目 README](../README.md)，下一步见 [路线图](ROADMAP.md)，换机器开发先看 [环境说明](ENVIRONMENT.md)。

## 账户与页面

- [API 与用户](design/sprint-1-api-users.md)：管理接口、认证边界和数据库迁移。
- [账户与会话](design/sprint-1-sessions-dashboard.md)：首次账户初始化、登录/退出和前端代理。
- [服务状态与概览](design/sprint-1-overview.md)：用户统计、UTC 日界线和刷新规则。

## 知识库

- 导入：[Markdown](design/sprint-2-markdown.md)、[PDF](design/sprint-2-pdf.md)、[公开网页](design/sprint-2-web-import.md)。
- 原文：[MinIO / S3 存储](design/sprint-2-object-storage.md)、[迁移与孤立对象清理](design/sprint-2-original-maintenance.md)。
- 索引：[Embedding / Qdrant](design/sprint-2-vector-index.md)、[持久化任务与重试](design/sprint-2-index-jobs.md)。
- [语义检索与引用问答](design/sprint-2-retrieval-qa.md)：API 与页面交互、引用核对、配置和安全边界。

## 验收与架构

- [用户长期记忆](design/sprint-3-long-memory.md)：手动 CRUD、版本冲突、配额、数据隔离及页面操作。
- [Redis 短期记忆](design/sprint-3-short-memory.md)：存储适配器、TTL、条目/活跃对话配额；尚未接入 API。
- [对话归属与元数据 API](design/sprint-3-conversations.md)：持久化所有者、创建幂等、额度与删除墓碑。
- [用户消息与 Redis 快照](design/sprint-3-messages.md)：持久化消息、幂等重放、删除栅栏、缓存故障回退及清理恢复。
- [对话与消息页面](design/sprint-3-conversation-ui.md)：显式创建/发送、原请求重试、删除确认与账户清理；不生成模型回复。
- [显式模型回复](design/sprint-3-model-replies.md)：上下文、请求仓储、次数额度、HTTP 与内置夹具执行器。
- [固定模型回复适配](design/sprint-3-reply-provider.md)：离线文本计数、保守费用上界和禁止重试的 HTTP 适配器。
- [回复货币预算](design/sprint-3-reply-budget.md)：整数费用规划、PostgreSQL 事务账本与保守结算、供应商启用门槛。
- [显式测试回复页面](design/sprint-3-reply-ui.md)：创建确认、原请求重试、历史恢复、状态轮询与取消。

- [回复预算领取与持久化配置停用](design/sprint-3-reply-dispatch.md)
- [预算回复供应商执行器](design/sprint-3-reply-executor.md)
- [显式付费回复部署与金额确认](design/sprint-3-paid-replies.md)
- [回复配置与金额账本运维](design/sprint-3-reply-operations.md)：只读配置查询、显式停用、用户日账本核对及分页明细。
- [受限 Agent 计划与用户授权](design/sprint-3-agent-plans.md)：固定只读步骤、精确授权、每计划上限、取消与一次性派发。
- [Agent 检索计划页面](design/sprint-3-agent-plan-ui.md)：固定预览、费用确认、执行结果、取消及原请求恢复。
- [模型 Agent 纯规划与两阶段费用授权](design/sprint-3-model-agent-budget.md)：严格查询建议、独立规划/执行确认、金额与模型/工具次数边界；尚未接入模型。
- [模型 Agent 检索证据与受限回答](design/sprint-3-model-evidence.md)：权威文本复核、稳定引用、严格 JSON 回答与空证据退款。
- [模型 Agent 检索/回答阶段仓储](design/sprint-3-model-execution-store.md)：独立费用确认、共享金额/次数账本、逐步领取与取消/删除保护。
- [模型规划请求仓储与事务预算](design/sprint-3-model-planning-store.md)：第一阶段报价、精确批准、金额/次数预留、一次性领取、建议保存、取消及删除。
- [工具调用预算与审计](design/sprint-3-tool-call-audit.md)：一次性请求 ID、每日次数预算、元数据审计及故障保留。

- [Sprint 3：工具、记忆与调度](design/sprint-3-tools-memory-scheduler.md)：工具、记忆、对话/消息，以及后续页面、调度与 MCP 边界。

- [数据库与部署验收](design/sprint-1-acceptance.md)：隔离 Compose、双入口 HTTP 和资源清理。
- [无头浏览器验收](design/sprint-1-browser-acceptance.md)：macOS / WSL / Linux 页面交互测试。
- [架构决策](architecture/0001-hexagonal-boundaries.md)：端口与适配器边界。
- [原始 v1.0 设计](design/Personal_AI_Augmentation_System_v1.0_Agent_Implementation_Design.md)：产品目标，不代表全部已实现。
- [仓库开发规则](../AGENTS.md)：代码风格、验证要求及提交格式。

详细设计保留各迭代协议和带日期的验收记录；当前进度以路线图为准。
