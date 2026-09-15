# Delivery roadmap

## Foundation（已完成）

- [x] Rust Workspace 与模块边界
- [x] Next.js Dashboard/PWA 壳
- [x] 可替换的 Storage、LLM、Tool 接口
- [x] Compose 本地依赖拓扑
- [x] 用户表初始迁移
- [x] 环境检查、CI 与统一开发命令

## Sprint 1（核心 HTTP 与无头浏览器验收已完成）

- [x] Axum API、配置加载、结构化日志与错误响应
- [x] PostgreSQL 用户仓储适配器及迁移执行器
- [x] 用户创建/查询 API 与部署管理 Bearer Token
- [x] 原始设计归档与 Sprint 1 详细实现设计
- [x] HTTP 路由测试与 PostgreSQL 集成测试/CI 入口
- [x] 最小身份认证：管理员配置密码，邮箱登录、持久化会话、退出
- [x] Dashboard 账户面板及真实服务状态
- [x] Dashboard 对接 `/api/healthz`、`/api/readyz` 与用户知识库今日概览（UTC）
- [x] 真实 PostgreSQL 集成测试和核心 Compose HTTP smoke test（API/Web/Nginx，含重启持久化）
- [x] WSL Chromium 浏览器交互验收（文件 input、面板刷新与退出清理；桌面/窄屏，独立于 HTTP smoke）

## Sprint 2

- [x] Markdown 导入、按用户隔离、去重与文档列表/详情
- [x] Markdown 文本解析与 Unicode 分块
- [ ] PDF/网页导入
- [ ] MinIO 原文适配器、Embedding、Qdrant adapter
- [ ] RAG 检索与知识问答

## Sprint 3

- [ ] Tool executor、Memory 与 Scheduler
- [ ] Redis short memory、PostgreSQL long memory
- [ ] MCP adapter 的首个只读工具

## Sprint 4

- [ ] RSS、去重、价值评分与 Daily Brief
- [ ] Skill Graph、能力评估与训练任务
