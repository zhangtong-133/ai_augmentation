# 交付路线图

已完成知识库闭环、工具、长期记忆、对话/用户消息页面和 Redis 快照缓存。显式回复已提供[持久化请求与夹具执行器](design/sprint-3-model-replies.md)及[测试回复页面](design/sprint-3-reply-ui.md)，包含双入口故障交互验收。默认关闭，部署者显式启用付费模式后可调用固定模型。已实现[费用预留/结算纯规划与供应商边界设计](design/sprint-3-reply-budget.md)，已落地金额账本与事务预留/结算，已提供[固定模型离线计数与单次发送适配](design/sprint-3-reply-provider.md)，已提供[预算凭据领取与持久化配置停用](design/sprint-3-reply-dispatch.md)，已提供[供应商内部执行器与故障恢复](design/sprint-3-reply-executor.md)，已接入[显式部署配置、事务预留入口与页面金额确认](design/sprint-3-paid-replies.md)。已提供[配置查询、显式停用与金额账本审计命令](design/sprint-3-reply-operations.md)。已提供[工具调用次数预算与持久化审计](design/sprint-3-tool-call-audit.md)，现有知识检索工具已接入一次性执行入口。已提供[固定只读 Agent 计划与精确用户授权](design/sprint-3-agent-plans.md)，每计划最多三次知识检索，复用次数审计并支持取消/删除及一次性派发。已提供[计划预览、费用授权与取消页面](design/sprint-3-agent-plan-ui.md)，支持原请求恢复及双入口浏览器验收。已提供[模型检索建议的纯规划与两阶段金额/次数授权](design/sprint-3-model-agent-budget.md)，已落地[第一阶段规划请求仓储与事务预算](design/sprint-3-model-planning-store.md)，已落地[检索/回答阶段预算与一次性领取](design/sprint-3-model-execution-store.md)，已落地[检索证据保存与受限回答协议](design/sprint-3-model-evidence.md)，已提供[固定模型适配器与一次性执行器](design/sprint-3-model-agent-executor.md)，已接入[显式部署配置、两阶段 HTTP 授权与页面](design/sprint-3-model-agent-app.md)，已提供[模型 Agent 配置查询、主动停用与共享账本核对](design/sprint-3-model-agent-operations.md)，已实现 [Scheduler 一次性提醒授权、取消与任务仓储](design/sprint-3-scheduler-store.md)，已接入[后台领取、租约恢复与幂等站内提醒投递](design/sprint-3-scheduler-delivery.md)，已接入[Scheduler 管理/提醒 API 与页面](design/sprint-3-scheduler-app.md)，已提供[Scheduler 运维查询与积压/租约核对](design/sprint-3-scheduler-operations.md)，已提供 [MCP 本地 stdio 只读知识检索桥接](design/sprint-3-mcp-stdio.md)。已提供 [MCP 宿主专属可撤销凭据与授权 API](design/sprint-3-mcp-credentials.md)。已提供 [MCP 凭据管理与费用授权页面](design/sprint-3-mcp-credential-ui.md)。已提供 [MCP 凭据只读运维核对](design/sprint-3-mcp-operations.md)。已提供 [RSS 订阅与只读采集纯规划边界](design/sprint-4-rss-collection.md)。已实现[受限 RSS 2.0 解析器与条目身份/去重纯函数](design/sprint-4-rss-parser.md)。已实现[RSS 订阅仓储、一次性采集授权与审计事务](design/sprint-4-rss-store.md)。已实现[RSS 公网传输与一次性执行器](design/sprint-4-rss-executor.md)。已接入[默认关闭的 RSS 管理与一次性采集 HTTP 接口](design/sprint-4-rss-http.md)。下一步实现 RSS 订阅与采集确认页面。阶段边界见 [Sprint 3 设计](design/sprint-3-tools-memory-scheduler.md)。

## 最近交付

- [x] Scheduler 只读运维命令：用户积压、过期租约、提醒记录结构核对与一致快照分页。

- [x] Scheduler 会话/CSRF 保护 API 与定时提醒页面：时间预览、精确确认、原请求重试、取消和提醒分页。

- [x] Scheduler 默认关闭的后台进程、跨进程租约领取/恢复、取消竞争保护和幂等站内提醒投递。

- [x] Scheduler 一次性站内提醒协议与仓储：精确授权、取消防复活、并发配额和事务持久化；后台执行尚未接入。

- [x] 模型 Agent 运维命令：分阶段配置查询、显式持久化停用、只读权限与共享金额账本核对。

- [x] 模型知识助手两次独立金额确认、后台执行、历史/引用、取消与原请求恢复；付费部署默认关闭。

- [x] 模型 Agent 两阶段一次性执行器与固定模型适配器：离线计数、顺序检索/回答、契约停用及失败不重发。

- [x] 模型 Agent 检索证据归属复核、稳定引用与严格回答协议；空证据跳过聊天并退还回答预留。

- [x] 模型 Agent 第二阶段独立确认、逐次费用/次数预留、顺序一次性领取及取消/超时退款。
- [x] 模型规划阶段仓储：精确授权、共享金额账本、模型次数预留、一次性领取、建议保存及取消/删除保护。
- [x] 模型 Agent 纯规划：严格只读查询建议、规划/执行两次金额确认及聊天/向量化/工具日次数检查；尚未接入模型。
- [x] Agent 检索计划页面：固定步骤预览、费用确认、状态/原文、取消及原请求恢复；覆盖双入口故障交互。
- [x] 固定只读 Agent 计划预览、精确授权、每计划三次上限和持久化执行结果；覆盖取消、版本变化与重启查重。
- [x] 知识检索工具接入一次性请求 ID、用户 UTC 日次数预算及持久化审计，双入口支持查询和重启后查重。
- [x] 回复配置查询、显式持久化停用和按用户 UTC 日核对金额账本；包含删除后的记录、精确汇总及差额报告。
- [x] 修复对象 CI 的 MinIO 镜像拉取失败，统一从固定官方源码构建并在验收开始时检查构建来源。
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
- [x] 工具执行次数预算与持久化审计（一次性 ID、失败保留、用户/日期隔离）
- [x] Agent 编排首阶段（固定只读检索计划、精确用户授权与每计划调用上限）
- [x] Agent 计划预览/授权页面与浏览器故障交互验收
- [x] 模型 Agent 纯规划、严格建议解码及两阶段金额/次数授权设计
- [x] 模型 Agent 规划阶段请求仓储与事务金额/次数预留
- [x] 模型 Agent 检索/回答阶段预算与一次性领取
- [x] 模型 Agent 检索证据保存与受限回答协议
- [x] 模型受限规划与答案生成内部执行器、固定供应商及已付费向量检索适配
- [x] 模型 Agent 显式部署配置、两阶段 HTTP 授权与页面接入
- [x] 模型 Agent 运维配置查询、主动停用与账本核对
- [x] Scheduler 内部授权、取消、租约与幂等投递
- [x] Scheduler 管理/提醒 API 与页面
- [x] Scheduler 运维查询与积压/租约核对
- [x] Redis 短期记忆适配器（用户/对话键隔离、TTL、条目/活跃对话配额及真实 Redis 测试）
- [x] 对话归属与元数据 API（认证、CSRF、会话撤销、删除墓碑、创建额度与幂等）
- [x] 用户消息接口与 Redis 快照缓存（持久化幂等、删除并发保护及失败恢复）
- [x] 对话/消息管理页面（显式发送、幂等重试、删除确认及账户隔离）
- [x] 显式回复的受限上下文规划器与费用/幂等/取消协议设计（未接入模型）
- [x] 回复请求仓储、事务次数额度、单次领取与取消/删除保护
- [x] 内置夹具执行器、过期恢复与显式回复 HTTP 操作（无模型调用）
- [x] 测试回复页面（显式请求、历史、轮询、取消与原请求重试）
- [x] 回复页面故障验收（丢失响应、取消重试、轮询停止、账户与晚到数据隔离）
- [x] 费用预留/结算纯规划与供应商接入边界设计
- [x] PostgreSQL 金额账本、事务预留与一次性结算
- [x] 固定模型正文计数、保守上下文上界与供应商单次发送适配（本地夹具验收，未启用付费入口）
- [x] 预算凭据一次性事务领取、不可变配置登记、价格有效期与持久化停用
- [x] 供应商内部执行器：预算领取复核、单次发送、结算、超时恢复与持久化停用
- [x] 货币预算与固定真实供应商适配（默认关闭、显式金额确认及部署配置，本地夹具分层验收）
- [x] 付费回复配置与账本运维（管理员命令、只读查询、显式停用及差额退出状态）
- [x] MCP adapter 的首个只读工具（本地 stdio、默认关闭、专用凭据与持久化审计）
- [x] MCP 宿主专属的可撤销最小权限凭据与授权 API
- [x] MCP 凭据管理、费用授权与一次性明文交付页面
- [x] MCP 凭据只读运维查询与授权数量核对

## Sprint 4

- [x] RSS 订阅与手动采集边界设计、不可变预览和精确同意纯规划
- [x] 受限 RSS 2.0 解析器与条目身份/去重纯函数
- [x] RSS 订阅仓储、一次性采集授权与持久化审计
- [x] RSS 公网传输与一次性执行器
- [x] RSS 会话/CSRF 管理与一次性采集 HTTP 接口
- [ ] RSS 订阅与采集确认页面
- [ ] RSS、去重、价值评分与 Daily Brief
- [ ] Skill Graph、能力评估与训练任务
