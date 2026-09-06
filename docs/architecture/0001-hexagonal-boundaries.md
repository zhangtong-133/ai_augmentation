# ADR-0001：外部依赖采用端口与适配器隔离

- 状态：Accepted
- 日期：2026-09-06

## 背景

系统需要长期演进，并允许 PostgreSQL、Qdrant、对象存储和模型提供商被替换。若领域逻辑直接依赖供应商 SDK，迁移成本和测试成本都会持续增加。

## 决策

核心业务仅依赖以下端口：

- `personal-ai-storage`：MetadataStore、VectorStore、ObjectStorage、MemoryStore
- `personal-ai-llm`：LlmProvider
- `personal-ai-tools`：Tool
- `personal-ai-agent-core`：Planner 与运行时协议

供应商 SDK 只允许出现在适配器层。应用入口负责组合具体实现。跨边界的数据结构属于端口 crate，不泄漏供应商类型。

## 结果

单元测试可以使用内存实现；更换供应商不影响领域 crate。代价是需要维护显式映射，并在新增能力时先设计稳定接口。
