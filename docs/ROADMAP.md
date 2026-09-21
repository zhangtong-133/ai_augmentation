# 交付路线图

当前已完成知识库导入、原文存储、索引任务、语义检索和引用问答 API。下一步是前端交互，不是重新实现后端检索链路。

## 下一步

- [ ] 文档列表增加索引提交按钮和任务进度/失败状态。
- [ ] 增加检索与问答入口，展示引用原文，区分证据不足和服务故障。
- [ ] 使用本地模型夹具补充无头 UI 验收；不在自动测试中调用付费模型。

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
- [x] macOS / WSL 无头 Chromium 交互验收（文件 input、面板刷新与退出清理；桌面/窄屏）

## Sprint 2

- [x] Markdown 导入、按用户隔离、去重与文档列表/详情
- [x] Markdown 文本解析与 Unicode 分块
- [x] PDF 导入（文本提取、原文件持久化、用户隔离与浏览器验收；不含 OCR）
- [x] 网页 URL 导入（公开静态 HTML、原文持久化、DNS/重定向限制及分层测试）
- [x] 公网网页成功导入浏览器用例与独立 `make browser-test-public` 入口；Mac/WSL Docker 与浏览器环境适配
- [x] 公网网页成功导入端到端验收通过（2026-09-16 Mac/OrbStack，完整双入口结果见网页导入设计）
- [x] MinIO / S3 原文适配器、导入接入、数据库引用与历史内联数据兼容
- [x] Embedding、Qdrant adapter 与按用户隔离的显式分批文档索引
- [x] 持久化索引任务、完整索引状态与自动重试（API 内后台执行器、租约恢复及每批三次上限）
- [x] 历史原文迁移与孤立对象清理（显式管理员命令、默认预览、回读校验和写入互斥保护）
- [x] 按用户隔离的语义检索 API、PostgreSQL 分块复核与双代理入口
- [x] 带核验引用的知识问答 API（独立启用、证据不足分支及受限结构化模型输出）
- [ ] 检索与问答交互 UI

## Sprint 3

- [ ] Tool executor、Memory 与 Scheduler
- [ ] Redis short memory、PostgreSQL long memory
- [ ] MCP adapter 的首个只读工具

## Sprint 4

- [ ] RSS、去重、价值评分与 Daily Brief
- [ ] Skill Graph、能力评估与训练任务
