# ADR-0001：外部依赖采用端口与适配器隔离

- 状态：Accepted
- 日期：2026-09-06

## 背景

系统需要长期演进，并允许 PostgreSQL、Qdrant、对象存储和模型提供商被替换。若领域逻辑直接依赖供应商 SDK，迁移成本和测试成本都会持续增加。

## 决策

核心业务仅依赖以下端口：

- `personal-ai-storage`：MetadataStore、VectorStore、ObjectStorage、MemoryStore
- `personal-ai-llm`：EmbeddingProvider、AnswerProvider 及模型请求/响应值类型
- `personal-ai-tools`：Tool
- `personal-ai-agent-core`：受限知识检索计划、模型两阶段规划、预算与一次性执行器协议

供应商 SDK 只允许出现在适配器层。应用入口负责组合具体实现。跨边界的数据结构属于端口 crate，不泄漏供应商类型。

## 结果

单元测试可以使用内存实现；更换供应商不影响领域 crate。代价是需要维护显式映射，并在新增能力时先设计稳定接口。

## 工程清理

早期无实现、无调用方的通用 `Planner` / `ContextBuilder`、`LlmProvider`、阶段字符串数组及其自证测试已移除。执行顺序和授权约束由具体计划与执行器及其行为测试验证。领域层移除未使用的 `TaskId`、`SkillId`、`TaskStatus`、`AgentState`；学习和定时提醒继续使用各自已经接入持久化的类型。`agent-core::BoxFuture` 仍供现有执行器接口使用。
