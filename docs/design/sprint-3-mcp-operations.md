# Sprint 3：MCP 凭据只读运维核对

提供管理员命令 mcp-operations，以显式用户 ID 核对授权数量和凭据元数据。不新增迁移、HTTP 接口或页面，不签发、撤销、续期或修复凭据。

## 使用

在已初始化的数据库上配置 DATABASE_URL，建议使用下述列级只读角色：

```sh
cargo run -p api-server --bin mcp-operations -- audit --user USER_UUID
cargo run -p api-server --bin mcp-operations -- audit --user USER_UUID --after CREDENTIAL_UUID
```

生产 API 镜像包含该命令，也可使用现有 Compose 服务：

```sh
docker compose exec api-server mcp-operations audit --user USER_UUID
```

必须提供合法用户 UUID；未知、重复、缺值参数或写操作在连接前拒绝。不存在的用户报错，存在但无凭据的用户返回全零汇总。数据库连接错误不回显 URL 或密码。命令仅读取 DATABASE_URL，不读取 MCP token、费用开关或其他执行器配置，也不执行迁移。

## 输出与退出状态

输出单个 JSON：

- user_id、snapshot_at_unix_ms：用户与本次只读快照时间，毫秒时间戳使用字符串。
- counts：total、active、expired、revoked、issued_last_24h、quota_used、inconsistent。
- quota_limit：当前签发限制 20；remaining_issuance 为 max(20 - quota_used, 0)。
- consistent 与 issues：全量核对结果，即使异常记录不在当前页也会报告。
- items：按 UUID 升序的最多 100 条元数据，含 id、状态、签发/到期/撤销时间及异常代码。
- next_cursor：还有后续记录时返回本页末尾 ID，否则为 null。

撤销状态优先；未撤销且已到期计入 expired，其余计入 active。active 是时间与撤销状态分类，不代表已检查客户端在线状态、索引是否可用或凭据是否被使用。最近 24 小时以 created_at > snapshot - 24h 计算；quota_used 是 active 与最近 24 小时签发记录的并集，不重复计数，与实际签发入口一致。撤销近期凭据不释放当天签发额度，旧的已撤销/到期记录不占额度。

退出码 0 表示本次元数据与额度核对正常；达到 20 条但未超过限制仍返回 0。超额或异常返回 2，并输出完整报告；输入或存储错误返回 1。以下情况记录异常：

| 代码 | 含义 |
| --- | --- |
| issuance_quota_exceeded | 全量 quota_used 超过签发限制 |
| invalid_credential_metadata | 至少一条凭据存在下列元数据异常 |
| unsupported_scope | scope 不为 knowledge_search |
| creation_in_future | 签发时间晚于快照 |
| invalid_validity_window | 到期不晚于签发，或期限超过 30 天 |
| revocation_before_creation | 撤销早于签发 |
| revocation_in_future | 撤销晚于快照 |

scope 与有效期也受现有数据库约束保护，检查用于发现异常导入或约束遭修改后的数据。工具不校验认证摘要内容，不读取宿主名称、token_digest、原始 token、知识正文或工具调用参数。它不能证明密钥安全，也不推断已派发请求是否完成。

## 只读权限与并发

最小查询权限示例（角色创建、登录方式及 CONNECT 权限由部署者配置）：

```sql
GRANT SELECT (id) ON users TO mcp_auditor;
GRANT SELECT (id, user_id, scope, created_at, expires_at, revoked_at)
  ON mcp_credentials TO mcp_auditor;
ALTER ROLE mcp_auditor SET default_transaction_read_only = on;
```

每次调用使用 REPEATABLE READ、READ ONLY 事务和 30 秒 SQL 超时；先建立 MVCC 快照，再一次性采集数据库时钟用于时间分类，汇总与当前页共用该时间和快照，避免正常并发提交被误报为未来时间。因此并发签发或撤销不会使单次报告的汇总和明细来自不同状态。分页之间是不同快照，发生并发修改时不承诺整次翻页的固定快照；可重新从首页核对。

## 验证与后续

集成测试覆盖列级只读角色运行真实命令、不读取宿主名或摘要、不更改数据、110 条分页、用户隔离、正常满额与异常超额、异常时间和并发撤销。make smoke 执行 PostgreSQL/CLI 集成并验证生产镜像内命令可运行。无付费模型调用或浏览器交互。

MCP 本地授权闭环已完成，Sprint 4 已提供 [RSS 订阅与只读采集边界设计](sprint-4-rss-collection.md)及纯规划模块；远程 MCP/OAuth 仍未启用。
