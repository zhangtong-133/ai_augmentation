# 文档索引

- [原文迁移与孤立对象清理](design/sprint-2-original-maintenance.md)：默认预览、回读校验、24 小时保留期、写入互斥与管理员维护命令。

- [持久化索引任务](design/sprint-2-index-jobs.md)：异步提交、连续分批进度、租约恢复、有界重试及隔离验收。

- [Embedding 与 Qdrant 索引](design/sprint-2-vector-index.md)：显式分批索引、用户/模型隔离、HTTP 契约和真实向量库验收。

- [MinIO / S3 原文存储](design/sprint-2-object-storage.md)：三种原文格式、私有桶、数据库引用、旧数据兼容与隔离验收。

- [网页 URL 导入](design/sprint-2-web-import.md)：公开静态网页抓取、正文提取、DNS/重定向限制及持久化。

- [PDF 导入](design/sprint-2-pdf.md)：文本提取、原文件持久化、解析限制与部署依赖。

- [WSL 无头浏览器验收](design/sprint-1-browser-acceptance.md)：独立 Playwright、双入口页面交互、测试隔离及报告。

- [真实数据库与部署验收](design/sprint-1-acceptance.md)：隔离 Compose、持久化集成测试、双入口 HTTP smoke test 与清理边界。

- [服务状态与今日概览](design/sprint-1-overview.md)：用户隔离统计、UTC 日界线、状态刷新及验证边界。

- [Markdown 导入与文档管理](design/sprint-2-markdown.md)：最新迭代，包含接口、分块、去重、用户隔离及存储阶段性差异。

- [登录会话与 Dashboard 设计](design/sprint-1-sessions-dashboard.md)：最新迭代，包含首个账户初始化、登录/退出、会话过期和前端代理配置。

- [原始 v1.0 设计文档](design/Personal_AI_Augmentation_System_v1.0_Agent_Implementation_Design.md)：完整保存用户附件，作为产品范围和架构基线。
- [Sprint 1 API 与用户模块设计](design/sprint-1-api-users.md)：本轮实现的接口、配置、认证边界、迁移与验证方式。
- [架构决策 ADR-0001](architecture/0001-hexagonal-boundaries.md)：端口与适配器隔离。
- [交付路线图](ROADMAP.md)：明确已完成和未完成能力。
- [环境基线](ENVIRONMENT.md)：本机工具链与外部服务检查记录。

原始设计是 v1 的目标，不代表当前仓库已实现全部能力。迭代实现与原始设计的细化差异记录在对应迭代文档中。

- [检索与引用问答分步设计](design/sprint-2-retrieval-qa.md)
