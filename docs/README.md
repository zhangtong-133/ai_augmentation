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

- [Sprint 3：工具、记忆与调度](design/sprint-3-tools-memory-scheduler.md)：受限工具、长期记忆、短期记忆适配器，以及后续对话 API、调度与 MCP 边界。

- [数据库与部署验收](design/sprint-1-acceptance.md)：隔离 Compose、双入口 HTTP 和资源清理。
- [无头浏览器验收](design/sprint-1-browser-acceptance.md)：macOS / WSL / Linux 页面交互测试。
- [架构决策](architecture/0001-hexagonal-boundaries.md)：端口与适配器边界。
- [原始 v1.0 设计](design/Personal_AI_Augmentation_System_v1.0_Agent_Implementation_Design.md)：产品目标，不代表全部已实现。
- [仓库开发规则](../AGENTS.md)：代码风格、验证要求及提交格式。

详细设计保留各迭代协议和带日期的验收记录；当前进度以路线图为准。
