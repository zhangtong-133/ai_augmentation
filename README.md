# Personal AI Augmentation System

个人 AI 知识库，使用 Rust / Axum、Next.js 和 PostgreSQL。已实现导入、原文存储、向量索引、语义检索和引用问答，页面支持索引进度、检索片段和引用核对。

## 当前能力

- 账户：邮箱密码登录、持久化 Cookie 会话、退出及用户隔离。
- Dashboard：服务状态、知识库统计、导入与文档预览、索引任务操作，以及检索与引用问答。
- 导入：Markdown、可提取文字的 PDF（≤ 5 MiB，不含 OCR）、公开 UTF-8 静态网页（≤ 1 MiB，拒绝内网地址）。
- 原文：默认保存在 PostgreSQL，可启用私有 MinIO / S3 桶；支持历史原文迁移与孤立对象清理，维护命令默认仅预览。
- 索引：Embedding / Qdrant、持久化任务、租约恢复及每批最多 3 次尝试；执行器目前运行在 API 进程内。
- 检索与问答：按用户隔离，召回分块经 PostgreSQL 复核；答案附核验引用，证据不足时明确返回。引用校验不保证答案语义正确。

导入不会自动索引或调用模型。已提供 [FileReader 工具](docs/design/file-reader-tool.md)，按当前用户分页读取已导入文档文本，不依赖模型或索引；与知识检索共享每日工具调用额度。已提供受限只读 `knowledge_search` 工具 API 及[调用次数预算与审计](docs/design/sprint-3-tool-call-audit.md)、用户手动管理的[长期记忆](docs/design/sprint-3-long-memory.md)，以及[对话与用户消息页面](docs/design/sprint-3-conversation-ui.md)（持久化消息、可选 Redis 快照缓存、幂等重试）。[显式回复页面](docs/design/sprint-3-reply-ui.md)支持请求、历史、轮询与取消；执行默认关闭。已提供[受限 Agent 计划 API](docs/design/sprint-3-agent-plans.md)：用户预览并授权后，最多顺序执行三次知识检索，支持持久化结果与取消。已提供[计划预览、费用确认与取消页面](docs/design/sprint-3-agent-plan-ui.md)，支持状态/原文及原请求恢复。模型规划已提供[内部固定模型适配器与两阶段一次性执行器](docs/design/sprint-3-model-agent-executor.md)，已接入[显式部署配置与两阶段费用页面](docs/design/sprint-3-model-agent-app.md)，默认关闭；已提供 [Scheduler 一次性提醒授权与仓储](docs/design/sprint-3-scheduler-store.md)，已接入[默认关闭的后台投递进程](docs/design/sprint-3-scheduler-delivery.md)，已提供[任务管理和提醒页面](docs/design/sprint-3-scheduler-app.md)及[只读运维核对命令](docs/design/sprint-3-scheduler-operations.md)，已收到的提醒支持[已读标记、归档与恢复](docs/design/reminder-inbox.md)。已提供 [RSS 手动采集的纯规划与授权边界](docs/design/sprint-4-rss-collection.md)，以及[受限 RSS 2.0 解析与去重](docs/design/sprint-4-rss-parser.md)，已提供[订阅持久化、一次性授权与审计事务](docs/design/sprint-4-rss-store.md)，已提供[公网传输与一次性执行器内部库](docs/design/sprint-4-rss-executor.md)，已接入[默认关闭的 RSS 管理与一次性采集 API](docs/design/sprint-4-rss-http.md)，已提供[订阅、条目与明确采集确认页面](docs/design/sprint-4-rss-ui.md)。可使用 [feed-operations](docs/design/sprint-4-rss-operations.md) 只读核对采集状态、额度与审计。已提供 [RSS 规则评分与 Daily Brief 纯规划库](docs/design/sprint-4-rss-brief-planning.md)，已实现[日报偏好与不可变计划仓储](docs/design/sprint-4-rss-brief-store.md)，已接入[日报 HTTP 与页面](docs/design/sprint-4-rss-brief-app.md)，支持偏好、显式生成与历史管理，并可[启用每日定时日报](docs/design/daily-brief-schedule.md)，由 scheduler 整理已保存条目。已实现[Skill Graph、自评与受限训练计划纯规划](docs/design/sprint-4-learning-planning.md)，已实现[技能图、自评与不可变训练计划仓储](docs/design/sprint-4-learning-store.md)，已接入[学习管理 HTTP、页面与显式训练结果](docs/design/sprint-4-learning-app.md)，支持技能前置关系、自评、计划历史和训练完成/取消记录。已提供[学习进度概览](docs/design/sprint-4-learning-progress.md)，查看当前自评覆盖、待记录训练及 UTC 今日记录用时。可使用 [learning-operations](docs/design/sprint-4-learning-operations.md) 只读核对学习图、计划/结果状态和额度。当前交付范围与后续边界见 [路线图](docs/ROADMAP.md)。

显式回复支持本地夹具和管理员配置的固定模型付费模式；使用前需设置价格有效期及额度，用户在页面确认金额。配置见 [付费回复部署设计](docs/design/sprint-3-paid-replies.md)；管理员可使用 [reply-operations](docs/design/sprint-3-reply-operations.md)查询配置、显式停用及核对用户日账本。模型助手对应使用 [model-agent-operations](docs/design/sprint-3-model-agent-operations.md)，分别管理规划和执行阶段。

已提供[模型 Agent 纯规划与两阶段费用授权](docs/design/sprint-3-model-agent-budget.md)：严格校验只读检索建议，分别确认规划、检索与回答的金额及次数。已落地[第一阶段请求仓储与事务预算](docs/design/sprint-3-model-planning-store.md)，支持精确批准、一次性领取、结算和取消；第二阶段仓储及模型执行入口尚待接入。

## 快速开始

需要 Rust 1.96+、Node.js 20.9+、npm 和可用的 Docker / Compose。macOS 的 PDF 提取使用 Linux API 容器；浏览器验收使用无头 Chromium，不占用前台。详见 [环境说明](docs/ENVIRONMENT.md)。

```bash
cp .env.example .env
# 编辑 .env：替换默认口令，并设置 API_AUTH_TOKEN（至少 32 个可见 ASCII 字符）
make env-check
make compose-config
make stack-up
```

`make compose-config` 校验示例配置；`make stack-up` 使用本地 `.env` 构建并启动完整服务栈。Web 默认位于 http://localhost:3000。首次使用需管理员创建用户并设置密码，见 [账户初始化](docs/design/sprint-1-sessions-dashboard.md)。管理 token 不得放进前端代码。

分别开发前后端时：

```bash
make infra-up
make web-install
make web-dev
# 另一个终端：导出 DATABASE_URL、API_AUTH_TOKEN 等配置后运行
cargo run -p api-server
```

本机 Rust 进程不自动加载 `.env`；数据库地址使用回环地址，本地 HTTP 登录需设置 `SESSION_COOKIE_SECURE=false`。数据库连接或迁移失败时 API 不启动。

## 可选能力

默认关闭，完整配置见 [.env.example](.env.example)。

| 能力 | 启用条件 | 接口或说明 |
|---|---|---|
| 外部原文存储 | `OBJECT_STORE_ENABLED=true`，配置桶及凭据 | [存储](docs/design/sprint-2-object-storage.md)、[迁移与清理](docs/design/sprint-2-original-maintenance.md)；不自动搬迁旧数据 |
| 索引与检索 | `KNOWLEDGE_INDEX_ENABLED=true`，配置模型、维度及 Qdrant | `POST/GET /api/documents/{id}/index-job`、`POST /api/knowledge/search` |
| 引用问答 | 已启用索引，再设置 `KNOWLEDGE_ANSWER_ENABLED=true` 和 `OPENAI_CHAT_MODEL` | `POST /api/knowledge/answer`；模型须支持严格结构化输出 |
| RSS 手动采集 | `RSS_COLLECTION_MODE=public` | [订阅管理、预览、明确确认和状态恢复](docs/design/sprint-4-rss-http.md)；默认 `disabled`，管理和预览仍可用 |
| 消息快照缓存 | `MESSAGE_CACHE_ENABLED=true`，配置 `REDIS_URL` | 固定 30 分钟 TTL；关闭或故障时从 PostgreSQL 读取，不丢失消息 |

知识库接口需要登录会话；POST 还需 `X-Requested-With: personal-ai`。同步分批索引接口仍保留，但不更新异步任务进度。协议和限制见 [索引任务](docs/design/sprint-2-index-jobs.md)、[检索与问答](docs/design/sprint-2-retrieval-qa.md)。模型超时或任务恢复可能重复计费，真实模型需单独评估。

## 工具接口

Sprint 3 工具入口：登录后 `GET /api/tools` 查看可用工具，`POST /api/tools/knowledge_search` 显式检索（需 `Idempotency-Key: UUID`，沿用索引配置与 CSRF 要求，可能产生模型费用）。REST 接口另可通过 [MCP 本地 stdio 桥接](docs/design/sprint-3-mcp-stdio.md)供可信宿主使用：默认关闭，显式启用后使用[宿主专属可撤销凭据](docs/design/sprint-3-mcp-credentials.md)，复用工具额度与审计。登录后可在 [MCP 宿主授权页面](docs/design/sprint-3-mcp-credential-ui.md)创建、查看和撤销凭据。管理员可使用 [mcp-operations](docs/design/sprint-3-mcp-operations.md) 只读核对凭据状态及签发额度。

固定 Agent 计划使用 `POST /api/conversations/{id}/agent-plans` 创建预览，再通过 `/{request}/approve` 确认指纹、次数和费用；详情与取消协议见 [计划设计](docs/design/sprint-3-agent-plans.md)。需要已有消息版本和已启用的知识检索。执行不会自动重试，每步计入同一工具日预算。

## 验证

```bash
make check
npm --prefix apps/web run lint
npm --prefix apps/web run typecheck
npm --prefix apps/web run build
```

隔离验收需要 Docker，结束后仅清理本次测试资源，保留构建缓存：

| 命令 | 范围 |
|---|---|
| `make smoke` | PostgreSQL/Redis、双入口 HTTP、消息幂等、缓存故障恢复及重启持久化 |
| `make smoke-objects` | 加测真实 MinIO、原文迁移与清理 |
| `make smoke-index` | 加测真实 Qdrant、本地模型夹具、索引/检索/问答和 Agent 计划双入口授权/重启查重 |
| `TEST_DATABASE_URL=… make test-feed-operations` | RSS 运维 CLI、元数据列权限与退出码（需要临时数据库建角色权限） |
| `TEST_DATABASE_URL=… make test-feeds` | RSS HTTP 私有订阅、精确确认、默认关闭和未知结果恢复 |
| `TEST_DATABASE_URL=… make test-agent` | 一次性 PostgreSQL 验证计划授权、事务预算、取消和故障边界 |
| `TEST_DATABASE_URL=… make test-model-agent` | 一次性 PostgreSQL 验证模型规划阶段事务金额/次数、领取、取消及查重 |
| `make browser-install` → `make browser-test` | 安装当前平台 Chromium，再执行无头 UI 验收 |
| `make browser-test-index` | 使用真实 Qdrant 和本地模型夹具验证索引、检索、问答 UI 及用户隔离 |
| `make browser-test-public` | 额外验证公网网页导入，需要 API 能直连公网 |

自动验收不调用真实付费模型；历史通过记录不代表当前环境或最新 CI 状态。

## 工程与文档

- `apps/`：API（含索引、回复与模型 Agent 后台执行器）、Web 和处理一次性提醒、定时日报的 scheduler。
- `crates/`：领域能力、端口及 PostgreSQL、S3、Qdrant、模型适配器。
- `infra/`、`scripts/`：部署配置与开发/验收脚本。
- [文档索引](docs/README.md)：设计、配置和验收边界。
- [AGENTS.md](AGENTS.md)：开发与提交规则，使用 `feat(knowledge): 中文描述` 等格式。

### 工程清理与旧部署

空转的 `worker` 占位程序及其 Compose 服务已移除；实际后台执行器位于 API，定时提醒由 scheduler 处理。旧部署更新后，原有 worker 容器会成为孤立容器；确认容器所属 Compose 项目及服务标签后，单独停止并移除该占位容器。首页已移除禁用问答框与过时的静态模块状态，功能状态以登录后的实际面板为准。

残留向量可使用[显式维护命令](docs/design/vector-maintenance.md)按用户和模型分页核对、清理；默认预览，执行前须确认数据库归属并停止写入。

可显式配置 [GitTool 本地只读提交历史](docs/design/git-log-tool.md)，让指定用户按仓库别名查询最近提交，复用工具调用额度与审计；默认关闭。
