# 交付路线图

已完成知识库闭环、工具、长期记忆、对话/用户消息 API 与管理页面，以及可选 Redis 快照缓存。显式回复已完成上下文规划、[持久化请求与次数额度](design/sprint-3-model-replies.md)，下一步接本地模型夹具执行器与显式 HTTP 操作；当前不生成模型回复。阶段边界见 [Sprint 3 设计](design/sprint-3-tools-memory-scheduler.md)。

## 最近交付

- [x] 文档列表增加索引提交按钮和任务进度/失败状态。
- [x] 增加检索与问答入口，展示引用原文，区分证据不足和服务故障。
- [x] 使用本地模型夹具补充无头 UI 验收；不在自动测试中调用付费模型。

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
- [x] 索引任务 UI：显式提交、进度轮询、失败重试和服务状态提示
- [x] 历史原文迁移与孤立对象清理（显式管理员命令、默认预览、回读校验和写入互斥保护）
- [x] 按用户隔离的语义检索 API、PostgreSQL 分块复核与双代理入口
- [x] 带核验引用的知识问答 API（独立启用、证据不足分支及受限结构化模型输出）
- [x] 检索与问答交互 UI（纯文本结果、引用片段、错误分流、账户切换清理）

## Sprint 3

- [x] 受限 Tool executor 与只读 `knowledge_search` API（白名单、会话隔离、超时和并发限制）
- [x] 用户显式管理 PostgreSQL 长期记忆（CRUD、页面、版本冲突、配额与隔离）
- [ ] Agent 编排、工具审计与调用预算
- [ ] Scheduler（授权、取消、租约与幂等）
- [x] Redis 短期记忆适配器（用户/对话键隔离、TTL、条目/活跃对话配额及真实 Redis 测试）
- [x] 对话归属与元数据 API（认证、CSRF、会话撤销、删除墓碑、创建额度与幂等）
- [x] 用户消息接口与 Redis 快照缓存（持久化幂等、删除并发保护及失败恢复）
- [x] 对话/消息管理页面（显式发送、幂等重试、删除确认及账户隔离）
- [x] 显式回复的受限上下文规划器与费用/幂等/取消协议设计（未接入模型）
- [x] 回复请求仓储、事务次数额度、单次领取与取消/删除保护
- [ ] 回复执行器、HTTP 操作、货币预算与页面
- [ ] MCP adapter 的首个只读工具

## Sprint 4

- [ ] RSS、去重、价值评分与 Daily Brief
- [ ] Skill Graph、能力评估与训练任务
