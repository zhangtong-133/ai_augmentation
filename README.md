# Personal AI Augmentation System

个人长期 AI 基础设施的 v1 工程。当前已支持账户会话、Markdown 导入及知识库概览；包含可编译的 Rust Workspace、Next.js Dashboard、清晰的外部依赖边界，以及本地服务编排。

核心部署验收运行 `make smoke`：构建独立 Compose 测试环境，执行真实数据库与双入口 HTTP 测试，结束后自动清理该次测试数据。前置条件和范围见 [验收设计](docs/design/sprint-1-acceptance.md)。

Dashboard 显示 API 存活/数据库就绪状态，以及当前用户的文档总数、文本块总数和今日导入量（UTC）；导入后自动刷新。接口与统计口径见 [今日概览设计](docs/design/sprint-1-overview.md)。

## 当前包含

知识库已支持登录后导入 Markdown、分页列表、原文与分块预览；设计与限制见 [Markdown 导入设计](docs/design/sprint-2-markdown.md)。本轮升级会话 Cookie 后需重新登录一次。

最新迭代已支持邮箱密码登录、持久化 Cookie 会话和 Dashboard 账户面板。首次使用需管理员为已有用户设置密码；操作步骤见 [登录会话与 Dashboard 设计](docs/design/sprint-1-sessions-dashboard.md)。本地 HTTP 启动 API 时设置 SESSION_COOKIE_SECURE=false（默认只允许 HTTPS Cookie）。

- `apps/api-server`：Axum API、健康/就绪检查、Bearer Token 鉴权、用户创建/查询
- `apps/worker`、`apps/scheduler`：后台进程运行壳
- `apps/web`：Next.js App Router + PWA manifest 的 Dashboard 壳
- `crates/*`：Domain、Agent Runtime、Knowledge、Learning、Storage、LLM、Tools、MCP 边界
- `compose.yaml`：Nginx、Web、三个 Rust 进程、PostgreSQL、Qdrant、Redis、MinIO
- `crates/storage-postgres`：SQLx 用户仓储与自动执行的版本化迁移
- `infra/postgres/init`：保留初始建表脚本，兼容首阶段数据库卷

业务 crate 不直接绑定 PostgreSQL、Qdrant、MinIO 或任一模型厂商。PostgreSQL 适配器已实现 `MetadataStore`；其他存储和模型适配器仍待实现。Rust 工具链基线为 1.96。

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
