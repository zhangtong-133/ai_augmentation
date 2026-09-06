# Sprint 1：API 与用户模块实现设计

后续迭代已新增密码/会话及 Dashboard 账户面板，见 [登录会话设计](sprint-1-sessions-dashboard.md)。下文保留本轮最初的范围说明。

日期：2026-09-06。基线为同目录的原始 v1.0 设计。本轮范围是可持久化的用户管理 API；Dashboard 仍是展示壳。

## 依赖与调用边界

HTTP 请求经过 Axum 路由、Bearer Token 校验、DTO 校验后调用 MetadataStore。应用入口将 storage-postgres 的 SQLx 适配器注入为 Arc<dyn MetadataStore>。领域类型不含 HTTP 或 SQLx 类型；storage crate 只定义端口。数据库 SQL 集中在新增的 storage-postgres crate 中。

生产入口不使用内存回退。测试通过端口注入内存仓储，不需要绑定 TCP 端口。配置、启动日志与请求完成日志采用 JSON；日志不记录 token、请求体或数据库 URL。

## 接口契约

| 方法与路径 | 鉴权 | 行为 |
|---|---|---|
| GET /healthz、/api/healthz | 无 | 200，进程存活 |
| GET /readyz、/api/readyz | 无 | 数据库 SELECT 1 成功为 200，否则 503 |
| POST /api/users | Bearer | 创建用户，201，返回用户与 Location 头 |
| GET /api/users/{uuid} | Bearer | 200，返回用户；不存在返回 404 |

创建请求为 JSON 对象，仅接受 email 和 display_name 两个字符串字段。响应包含 id、email、display_name。邮箱去除首尾空白并转小写，最长 254 字节，要求一个 @ 且两侧非空，不允许空白或控制字符。这是应用输入约束，不是邮箱可达性验证。显示名称去除首尾空白，1–100 个 Unicode 字符，禁止控制字符。请求体上限 16 KiB。

错误使用统一 JSON 结构：

```json
{"error":{"code":"user_exists"}}
```

状态码：401 为无效管理凭证，400 为非法字段/UUID/损坏 JSON，413 为超大请求体，415 为非 JSON 内容类型，422 为 JSON 字段类型不符，404 为未找到，405 为方法不支持，409 为重复邮箱或 ID，503 为存储不可用。数据库细节不返回客户端。

## 认证范围

API_AUTH_TOKEN 必须配置为至少 32 个可见 ASCII 字符，建议使用 openssl rand -hex 32。请求使用 Authorization: Bearer <token>。token 比较使用常量时间字节比较（长度可见）。token 是拥有用户管理能力的部署管理凭证，尚无用户密码、注册登录、角色、租户隔离、会话或限流。不将它注入浏览器。

本机默认监听 127.0.0.1；Compose 显式监听容器内 0.0.0.0。当前 Compose 是本地开发配置；在公网部署前需要 TLS、正式会话认证和网络访问限制。

## 配置与生命周期

| 环境变量 | 行为 |
|---|---|
| API_HOST | 默认 127.0.0.1，使用 IP 地址；IPv6 用方括号包裹 |
| API_PORT | 默认 8080 |
| DATABASE_URL | 必填；本机使用 127.0.0.1，容器使用 postgres |
| API_AUTH_TOKEN | 必填管理 token |
| RUST_LOG | tracing 过滤器，默认 info |

本机 cargo run 不自动加载 .env。Compose 使用项目 .env 替换环境变量。API 先检查配置，再创建最大 5 连接的池（获取连接超时 5 秒），执行 SQLx 迁移后监听 HTTP。数据库不可达或迁移失败时退出；SIGINT/SIGTERM 触发优雅关闭。

## 数据与迁移

users 表使用 UUID 主键、email、display_name、created_at、updated_at。当前只有创建和查询，没有更新/删除。MetadataStore::save_user 在该适配器中的语义是插入，重复 ID/邮箱返回 Conflict，不覆盖数据。

迁移位于 crates/storage-postgres/migrations，并嵌入二进制，由 SQLx 管理版本、校验和与迁移锁：

1. 0001 保持与首阶段 Compose init 建表一致；已有数据库卷也可接管到 SQLx 迁移历史。
2. 0002 增加 lower(email) 唯一索引，以保证并发下大小写邮箱仍不重复。若旧数据已有大小写重复，迁移会失败并保留数据，需要人工处理重复记录。

已执行迁移不修改；未来变化增加新迁移。旧 infra 初始化脚本暂保留兼容性，后续 schema 变更只追加 SQLx 迁移。数据库 init 脚本仅在空数据卷启动时执行。

## 本地验证

先启动 PostgreSQL 并配置本机 DATABASE_URL：

```bash
export DATABASE_URL='postgres://personal_ai:实际密码@127.0.0.1:5432/personal_ai'
export API_AUTH_TOKEN="$(openssl rand -hex 32)"
cargo run -p api-server
```

在另一个终端设置相同 API_AUTH_TOKEN：

```bash
curl --noproxy '*' -i http://127.0.0.1:8080/api/readyz
curl --noproxy '*' -i http://127.0.0.1:8080/api/users \
  -H "Authorization: Bearer $API_AUTH_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"email":"me@example.com","display_name":"我"}'
# 将返回的 id 替换到路径：
curl --noproxy '*' http://127.0.0.1:8080/api/users/返回的UUID \
  -H "Authorization: Bearer $API_AUTH_TOKEN"
```

常规测试：make check。数据库集成测试使用专用、可丢弃数据库（会迁移并写入随机测试用户）：

```bash
TEST_DATABASE_URL='postgres://personal_ai:测试密码@127.0.0.1:5432/personal_ai_test' make test-postgres
```

CI 已配置独立 PostgreSQL 16 service，显式运行该测试；普通 cargo test 将它显示为 ignored，避免将缺少数据库误报成通过。测试覆盖重复迁移、重连读取、唯一约束和未找到记录。

## 后续

Dashboard 连接健康状态与用户会话；明确本地单用户登录体验，再实现密码/会话或外部身份提供商。之后进入知识导入、模型与向量存储适配器。当前 Agent/worker/scheduler 仍是协议或进程壳。
