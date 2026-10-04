# 文档索引

当前能力与启动方式见 [项目 README](../README.md)，下一步见 [路线图](ROADMAP.md)，换机器开发先看 [环境说明](ENVIRONMENT.md)。

## 账户与页面

- [独立本地模型部署](design/local-model.md)：官方 llama.cpp/Qwen、精确授权与一次执行、6 GiB 游戏显存预算、真实模型验收和 vLLM 替换边界。

- [学习核验临时正文](design/learning-text-stream.md)：已交付默认关闭的 Pub/Sub 通道、逐段来源复核与纯文本页面，含分阶段验收记录。

- [v1 后续交付计划](design/v1-completion-plan.md)：订阅评分阅读闭环及能力评估、流式/本地适配、可选 API 和最终集成的验收门槛。

- [评分私有阅读投影](design/rss-value-reading.md)：从已完成的冻结评分映射摘要、原文和两种分数，失效后拒绝正文。

- [RSS 评分授权与结果页面](design/rss-value-ui.md)：连接/模型选择、精确双重同意、未知结果核对和私有评分展示。

- [RSS 用户评分 HTTP](design/rss-value-http.md)：会话范围内的预览、精确批准、取消、私有结果和审计；网页不执行模型。

- [RSS 本地订阅评分](design/rss-value-local.md)：本地身份锁、订阅流式请求及显式预览/批准/执行/查询/取消命令。

- [RSS 订阅评分内部执行](design/rss-value-execution.md)：单次领取、身份复核、发送标记、结果校验与取消/超时恢复。

- [订阅连接管理页面](design/subscription-connections-ui.md)：列表、详情、明确撤销、版本冲突与结果核对。

- [订阅连接管理 HTTP](design/subscription-connections-http.md)：用户会话保护下的私有列表、详情和版本化撤销。

- [订阅连接归属与撤销](design/subscription-connections.md)：显式绑定应用用户、查询和撤销，并作废相关 RSS 评分授权。

- [ChatGPT 订阅本地接入](design/chatgpt-local.md)：独立 OAuth 登录、账户模型列表和显式订阅调用命令；尚未接入网页。

- [API 与用户](design/sprint-1-api-users.md)：管理接口、认证边界和数据库迁移。
- [账户与会话](design/sprint-1-sessions-dashboard.md)：首次账户初始化、登录/退出和前端代理。
- [服务状态与概览](design/sprint-1-overview.md)：用户统计、UTC 日界线和刷新规则。

## 知识库

- 导入：[Markdown](design/sprint-2-markdown.md)、[PDF](design/sprint-2-pdf.md)、[公开网页](design/sprint-2-web-import.md)。
- 原文：[MinIO / S3 存储](design/sprint-2-object-storage.md)、[迁移与孤立对象清理](design/sprint-2-original-maintenance.md)。
- [外部原文备份恢复](design/originals-recovery.md)：快照引用、私有文件归档、完整性校验、条件创建与数据库开放前逐字节核对。
- 索引：[Embedding / Qdrant](design/sprint-2-vector-index.md)、[持久化任务与重试](design/sprint-2-index-jobs.md)。
- [语义检索与引用问答](design/sprint-2-retrieval-qa.md)：API 与页面交互、引用核对、配置和安全边界。

## 验收与架构

- [本地 RSS 价值评分](design/rss-value-local-model.md)：独立本地计算同意、精确目标、单次派发、页面和私有阅读；真实模型及删除/恢复三阶段已交付。

- [PostgreSQL 备份恢复](design/postgres-recovery.md)：同一快照归档、迁移/SHA-256 校验、空库事务恢复与执行隔离、应用登录/用户隔离/删除墓碑演练。
- [部署数据库只读诊断](design/deployment-check.md)：显式 current/recovery 检查、迁移校验、旧执行状态和外部原文核对。

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
- [Scheduler 运维核对](design/sprint-3-scheduler-operations.md)：只读积压/租约查询、提醒一致性及最小列权限。
- [Scheduler 管理与提醒页面](design/sprint-3-scheduler-app.md)：会话保护、精确时间确认、原请求重试及提醒列表。
- [Scheduler 租约与提醒投递](design/sprint-3-scheduler-delivery.md)：后台进程、租约恢复、取消竞争和幂等站内投递。
- [Scheduler 授权与任务仓储](design/sprint-3-scheduler-store.md)：一次性站内提醒、精确确认、取消和持久化。
- [模型 Agent 配置与账本运维](design/sprint-3-model-agent-operations.md)：分阶段配置查询、显式停用与共享金额核对。
- [回复配置与金额账本运维](design/sprint-3-reply-operations.md)：只读配置查询、显式停用、用户日账本核对及分页明细。
- [受限 Agent 计划与用户授权](design/sprint-3-agent-plans.md)：固定只读步骤、精确授权、每计划上限、取消与一次性派发。
- [Agent 检索计划页面](design/sprint-3-agent-plan-ui.md)：固定预览、费用确认、执行结果、取消及原请求恢复。
- [模型 Agent 纯规划与两阶段费用授权](design/sprint-3-model-agent-budget.md)：严格查询建议、独立规划/执行确认、金额与模型/工具次数边界；尚未接入模型。
- [模型 Agent 部署与两阶段授权页面](design/sprint-3-model-agent-app.md)：默认关闭、精确金额与次数确认、后台执行、历史与取消恢复。
- [模型 Agent 固定供应商与一次性执行器](design/sprint-3-model-agent-executor.md)：两次独立授权、离线计数、顺序执行、契约停用及失败不重发。
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

- [Sprint 3：MCP 本地 stdio 桥接](design/sprint-3-mcp-stdio.md)：默认关闭的只读检索与费用边界。

- [Sprint 3：MCP 宿主专属凭据](design/sprint-3-mcp-credentials.md)：最小权限、有效期、签发/撤销 API 与密码重设联动。

- [Sprint 3：MCP 凭据管理页面](design/sprint-3-mcp-credential-ui.md)：费用授权、一次性明文展示、故障核对与撤销。

- [Sprint 3：MCP 凭据运维核对](design/sprint-3-mcp-operations.md)：列级只读元数据查询、全量额度核对与分页。

- [Sprint 4：RSS 订阅与只读采集](design/sprint-4-rss-collection.md)：已实现纯规划与同意校验，明确后续 SSRF 及一次性执行边界。

- [Sprint 4：受限 RSS 2.0 解析与去重](design/sprint-4-rss-parser.md)：离线 XML/HTML 边界、GUID/链接身份、批次去重及账户/订阅隔离。

- [Sprint 4：RSS 订阅仓储与一次性采集事务](design/sprint-4-rss-store.md)：订阅版本、精确同意、并发/额度控制、原子条目写回及私有审计。

- [Sprint 4：RSS 公网传输与一次性执行器](design/sprint-4-rss-executor.md)：DNS 公网检查、TLS/连接固定、禁止重试、进程并发和未知结果边界；包含 RSS CI 失败排查。

- [Sprint 4：RSS 管理与一次性采集 HTTP 接口](design/sprint-4-rss-http.md)：会话/CSRF、默认关闭、精确确认、私有状态与显式恢复，以及 Next.js 双入口代理。

- [Sprint 4：RSS 订阅与采集确认页面](design/sprint-4-rss-ui.md)：订阅/条目/历史分页、来源确认、结果核对与双入口浏览器验收。

- [Sprint 4：RSS 只读运维核对与状态诊断](design/sprint-4-rss-operations.md)：元数据列权限、只读一致快照、额度/超时提示与审计核对。

- [Sprint 4：RSS 规则评分与 Daily Brief 纯规划](design/sprint-4-rss-brief-planning.md)：显式 UTC 窗口、关键词解释、稳定排名、保守去重与不可变摘要。

- [Sprint 4：Daily Brief 偏好与不可变计划仓储](design/sprint-4-rss-brief-store.md)：偏好版本、幂等快照、事务额度、来源删除清理及历史分页。

- [Sprint 4：Daily Brief HTTP 与页面](design/sprint-4-rss-brief-app.md)：会话/CSRF、显式生成、历史与删除、原请求核对及双入口验收。

- [Sprint 4：Skill Graph、自评与训练计划纯规划](design/sprint-4-learning-planning.md)：先修 DAG 校验、版本化自评、明确阻塞原因与确定性时间预算。

- [Sprint 4：技能图、自评与不可变训练计划仓储](design/sprint-4-learning-store.md)：图版本与 CAS、计划/任务原子写入、来源删除清理和持久化额度。

- [Sprint 4：学习管理 HTTP、页面与显式训练结果](design/sprint-4-learning-app.md)：技能管理、自评、计划历史、幂等结果及删除清理。

- [Sprint 4：学习数据只读运维核对与额度诊断](design/sprint-4-learning-operations.md)：元数据列权限、一致快照、图/版本/结果状态检查及持久化额度。

- [Sprint 4：学习进度概览](design/sprint-4-learning-progress.md)：只读当前自评覆盖、现存训练结果与 UTC 今日统计，包含页面刷新及故障隔离。

- [FileReader 只读文档工具](design/file-reader-tool.md)：已导入文档的无模型分页读取、用户隔离、次数审计及 MCP 范围隔离。

- [每日定时日报](design/daily-brief-schedule.md)：按用户显式启用，UTC 调度、单日去重及生成/配置原子提交。

- [提醒已读与归档](design/reminder-inbox.md)：收件箱、版本控制、恢复与投递记录保留。

- [残留向量显式维护](design/vector-maintenance.md)：默认预览、按用户/模型清理、跨存储存在性核对与安全重跑。

- [GitTool 本地只读提交历史](design/git-log-tool.md)：显式仓库映射、进程限制、用户隔离和调用审计。

- [WebSearch 显式搜索](design/web-search-tool.md)：固定 SearXNG 端点、逐次查询分享同意、搜索页面、受限结果与一次性审计。

- [自动 RSS 授权与时段规划](design/rss-schedule-planning.md)：有期限精确同意、频率/窗口边界与时段去重标识；尚未接入执行。

- [周期 RSS 采集授权仓储](design/rss-schedule-store.md)：不可变预览、同意/取消、订阅变更作废、到期与私有审计；尚未接入执行。

- [周期 RSS 单次领取与内部执行器](design/rss-schedule-execution.md)：持久化时段去重、共享额度、发送复核和结果写回；尚未自动运行。

- [周期 RSS 后台运行](design/rss-schedule-runner.md)：跨用户内部扫描、超时恢复、独立循环与默认关闭的部署配置。

- [周期 RSS 用户授权 HTTP](design/rss-schedule-http.md)：不可变预览、明确同意、取消及私有历史/审计，配置页面已接入。

- [周期 RSS 配置与同意页面](design/rss-schedule-ui.md)：有限期限和频率、精确同意、取消/审计、原请求恢复及双入口浏览器验收。

- [RSS 模型价值评分协议](design/rss-value-protocol.md)：冻结候选与最小分享、严格完整返回和明确放弃评分；费用授权及执行仍待实现。

- [RSS 评分用量授权仓储](design/rss-value-reviews.md)：API/订阅分离、不可变预览、精确同意及生命周期；实际订阅登录/调用待接入。
