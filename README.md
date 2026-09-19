# Personal AI Augmentation System

个人长期 AI 基础设施的 v1 工程。当前已支持账户会话、Markdown/PDF/网页 URL 导入及知识库概览；包含可编译的 Rust Workspace、Next.js Dashboard、清晰的外部依赖边界，以及本地服务编排。

核心部署验收运行 `make smoke`：构建独立 Compose 测试环境，执行真实数据库与双入口 HTTP 测试，结束后自动清理该次测试数据。前置条件和范围见 [验收设计](docs/design/sprint-1-acceptance.md)。

浏览器交互验收：在当前机器先运行 `make browser-install`，再运行 `make browser-test`。支持 WSL/Linux 与 macOS 无头 Chromium；`make browser-test-public` 额外验证真实公网网页成功导入，需要 API 容器能直连公网。详见 [浏览器验收设计](docs/design/sprint-1-browser-acceptance.md)。

Dashboard 显示 API 存活/数据库就绪状态，以及当前用户的文档总数、文本块总数和今日导入量（UTC）；导入后自动刷新。接口与统计口径见 [今日概览设计](docs/design/sprint-1-overview.md)。

PDF 支持可提取文字的文件（最多 5 MiB），原文件默认存 PostgreSQL，可启用 MinIO / S3；扫描件需先 OCR。Linux 本机 API 需安装 `poppler-utils`、`util-linux`，Compose 镜像已包含；macOS 使用 Linux API 容器运行 PDF 提取。详见 [PDF 导入设计](docs/design/sprint-2-pdf.md)。

网页 URL 导入支持公开 UTF-8 静态 HTML（最多 1 MiB），保留原 HTML 和最终来源，拒绝内网地址；详见 [网页导入设计](docs/design/sprint-2-web-import.md)。

## 当前包含

知识库已支持登录后导入 Markdown/PDF/网页 URL、分页列表、原文与分块预览；设计与限制见 [Markdown 导入设计](docs/design/sprint-2-markdown.md)。本轮升级会话 Cookie 后需重新登录一次。

最新迭代已支持邮箱密码登录、持久化 Cookie 会话和 Dashboard 账户面板。首次使用需管理员为已有用户设置密码；操作步骤见 [登录会话与 Dashboard 设计](docs/design/sprint-1-sessions-dashboard.md)。本地 HTTP 启动 API 时设置 SESSION_COOKIE_SECURE=false（默认只允许 HTTPS Cookie）。

- `apps/api-server`：Axum API、健康/就绪检查、Bearer Token 鉴权、用户创建/查询
- `apps/worker`、`apps/scheduler`：后台进程运行壳
- `apps/web`：Next.js App Router + PWA manifest 的 Dashboard 壳
- `crates/*`：Domain、Agent Runtime、Knowledge、Learning、Storage、LLM、Tools、MCP 边界
- `compose.yaml`：Nginx、Web、三个 Rust 进程、PostgreSQL、Qdrant、Redis、MinIO
- `crates/storage-postgres`：SQLx 用户仓储与自动执行的版本化迁移
- `infra/postgres/init`：保留初始建表脚本，兼容首阶段数据库卷

业务 crate 不直接绑定 PostgreSQL、Qdrant、MinIO 或任一模型厂商。PostgreSQL 适配器已实现 `MetadataStore` / `DocumentStore`，S3 适配器实现 `ObjectStorage`；Embedding 与 Qdrant 适配器已接入显式分批索引，聊天模型和 RAG 仍待实现。Rust 工具链基线为 1.96。

## 快速开始

```bash
cp .env.example .env
make env-check
make check
npm --prefix apps/web ci
npm --prefix apps/web run dev
```

API 启动（先启动 PostgreSQL；本机进程读取环境变量，不会自动加载 .env）：

```bash
export DATABASE_URL='postgres://personal_ai:替换为实际密码@127.0.0.1:5432/personal_ai'
export API_AUTH_TOKEN="$(openssl rand -hex 32)"
cargo run -p api-server
# 另一个终端
curl --noproxy '*' http://127.0.0.1:8080/healthz
```

本地数据服务：

```bash
make compose-config
make infra-up
```

当前主机只有旧版 `docker-compose` 时，`scripts/compose.sh` 会自动兼容；推荐最终安装 Docker Compose v2 插件。首次启动前务必修改 `.env` 中的默认口令。

完整 Compose 启动 API 时还需要在 `.env` 设置 `API_AUTH_TOKEN`（至少 32 个可见 ASCII 字符）。该 token 是个人部署的管理凭证，不是用户登录会话，也不应放进前端代码。数据库连接或迁移失败时 API 不启动。

文档入口：[文档索引](docs/README.md)、[原始 v1.0 设计](docs/design/Personal_AI_Augmentation_System_v1.0_Agent_Implementation_Design.md)、[Sprint 1 详细实现设计](docs/design/sprint-1-api-users.md)。接口与可执行示例见详细实现设计。

## 工程地图

```text
apps/       可部署进程与 Web 入口
crates/     领域能力及端口（ports）
infra/      服务编排与初始化脚本
docs/       架构决策和环境基线
scripts/    跨平台开发入口
```

下一步优先级见 [`docs/ROADMAP.md`](docs/ROADMAP.md)。设计边界见 [`docs/architecture/0001-hexagonal-boundaries.md`](docs/architecture/0001-hexagonal-boundaries.md)。

### MinIO 原文存储

设置 `OBJECT_STORE_ENABLED=true` 并配置 `.env.example` 中的对象存储 endpoint、bucket、region 与凭据后，新导入的 Markdown/PDF/网页原文写入私有桶；旧数据库原文继续可读。`make infra-up` 自动创建本地桶。迁移 `0007` 由 API 启动时执行；启动不会自动搬迁旧数据。详见 [原文存储设计](docs/design/sprint-2-object-storage.md)。

运行 `make smoke-objects` 可在独立 PostgreSQL/MinIO 环境验证原文、隔离、失败重试和双入口 HTTP 持久化；也可运行 `node scripts/smoke.mjs --objects --browser` 追加已有浏览器验收。

原文存储维护已提供默认只预览的 `object-maintenance` 命令，支持历史内联原文迁移与孤立对象清理。执行前须确认专用桶、所有写入器版本和备份策略，详见 [原文维护设计](docs/design/sprint-2-original-maintenance.md)。不会自动迁移或清理现有数据。

### 文档向量索引

配置模型、维度和 Qdrant 后设置 `KNOWLEDGE_INDEX_ENABLED=true`，已登录用户可通过 `POST /api/documents/{id}/index-job` 提交持久化任务，并通过同一路径的 GET 查询完整进度。后台每批最多 16 块，支持重启恢复和每批最多 3 次尝试；失败任务可再次 POST 从确认进度续传，已完成任务重复提交不会再次调用模型。POST 必须携带 `X-Requested-With: personal-ai`。导入不会自动调用模型，超时或恢复可能重复计费；当前没有页面按钮或问答接口。详见 [任务设计](docs/design/sprint-2-index-jobs.md)。

旧的 `POST /api/documents/{id}/index?offset=0` 同步分批接口继续支持，但不会更新任务进度。配置与接口细节见 [向量索引设计](docs/design/sprint-2-vector-index.md)。

`make smoke-index` 运行真实 Qdrant、确定性本地 Embedding HTTP 夹具与双入口验收，不调用外部模型。

启用索引后可调用 `POST /api/knowledge/search`，请求体为 `{"query":"问题","limit":5}`；需要会话 Cookie 与 `X-Requested-With: personal-ai`。返回当前用户的核验分块、标题、来源与得分，最多 20 条。检索不会自动索引文档，进行中的索引可能只返回部分分块。详见 [检索与问答设计](docs/design/sprint-2-retrieval-qa.md)。
