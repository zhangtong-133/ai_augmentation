# Personal AI Augmentation System

个人 AI 知识库，使用 Rust / Axum、Next.js 和 PostgreSQL。已实现导入、原文存储、向量索引、语义检索和引用问答，页面支持索引进度、检索片段和引用核对。

## 当前能力

- 账户：邮箱密码登录、持久化 Cookie 会话、退出及用户隔离。
- Dashboard：服务状态、知识库统计、导入与文档预览、索引任务操作，以及检索与引用问答。
- 导入：Markdown、可提取文字的 PDF（≤ 5 MiB，不含 OCR）、公开 UTF-8 静态网页（≤ 1 MiB，拒绝内网地址）。
- 原文：默认保存在 PostgreSQL，可启用私有 MinIO / S3 桶；支持历史原文迁移与孤立对象清理，维护命令默认仅预览。
- 索引：Embedding / Qdrant、持久化任务、租约恢复及每批最多 3 次尝试；执行器目前运行在 API 进程内。
- 检索与问答：按用户隔离，召回分块经 PostgreSQL 复核；答案附核验引用，证据不足时明确返回。引用校验不保证答案语义正确。

导入不会自动索引或调用模型。已提供受限只读 `knowledge_search` 工具 API、用户手动管理的[长期记忆](docs/design/sprint-3-long-memory.md)，以及[对话与用户消息 API](docs/design/sprint-3-messages.md)（持久化消息、可选 Redis 快照缓存）。尚无对话页面、模型回复、自动工具调用或定时任务。下一步见 [路线图](docs/ROADMAP.md)。

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
| 消息快照缓存 | `MESSAGE_CACHE_ENABLED=true`，配置 `REDIS_URL` | 固定 30 分钟 TTL；关闭或故障时从 PostgreSQL 读取，不丢失消息 |

知识库接口需要登录会话；POST 还需 `X-Requested-With: personal-ai`。同步分批索引接口仍保留，但不更新异步任务进度。协议和限制见 [索引任务](docs/design/sprint-2-index-jobs.md)、[检索与问答](docs/design/sprint-2-retrieval-qa.md)。模型超时或任务恢复可能重复计费，真实模型需单独评估。

## 工具接口

Sprint 3 工具入口：登录后 `GET /api/tools` 查看可用工具，`POST /api/tools/knowledge_search` 显式检索（沿用索引配置与 CSRF 要求，可能产生模型费用）。这是内部 REST 接口，不是 MCP 服务；边界见 [工具设计](docs/design/sprint-3-tools-memory-scheduler.md)。

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
| `make smoke-index` | 加测真实 Qdrant、本地模型夹具、索引/检索/问答 |
| `make browser-install` → `make browser-test` | 安装当前平台 Chromium，再执行无头 UI 验收 |
| `make browser-test-index` | 使用真实 Qdrant 和本地模型夹具验证索引、检索、问答 UI 及用户隔离 |
| `make browser-test-public` | 额外验证公网网页导入，需要 API 能直连公网 |

自动验收不调用真实付费模型；历史通过记录不代表当前环境或最新 CI 状态。

## 工程与文档

- `apps/`：API、Web 与 worker/scheduler 进程壳。
- `crates/`：领域能力、端口及 PostgreSQL、S3、Qdrant、模型适配器。
- `infra/`、`scripts/`：部署配置与开发/验收脚本。
- [文档索引](docs/README.md)：设计、配置和验收边界。
- [AGENTS.md](AGENTS.md)：开发与提交规则，使用 `feat(knowledge): 中文描述` 等格式。
