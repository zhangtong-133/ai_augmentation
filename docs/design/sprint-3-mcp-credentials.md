# Sprint 3：MCP 宿主专属凭据与授权 API

MCP 桥接改用独立的只读检索凭据，不再接收或转发网页登录 Cookie。凭据仅允许当前用户的 knowledge_search，不能访问账户、知识原文管理、工具审计、Agent、Scheduler、凭据管理或管理员接口。没有新增远程 MCP 服务或 OAuth 协议。

## 创建与管理

通过已登录的同源客户端调用以下接口。写操作要求 `X-Requested-With: personal-ai`，沿用用户会话与 CSRF 边界；MCP 凭据自身不能管理凭据。

| 接口 | 行为 |
| --- | --- |
| POST /api/mcp/credentials | 显式授权并签发凭据，返回 201 |
| GET /api/mcp/credentials | 当前用户最多 100 条记录，有效凭据优先，其余按创建时间倒序 |
| POST /api/mcp/credentials/{id}/revoke | 撤销自己的凭据，重复撤销仍返回 200；其他用户的 ID 返回 404 |

创建请求拒绝额外字段，示例：

```json
{
  "host_name": "我的本地 MCP 客户端",
  "expires_in_days": 7,
  "acknowledge_embedding_cost": true
}
```

宿主名去除首尾空白，1–80 字符且不含控制字符；期限为 1–30 天，没有永久凭据或自动续期。用户需知道检索可能产生向量模型费用，明确授权后创建。响应含 `credential` 元数据和仅此次返回的 `token`，设置 Cache-Control: no-store。token 格式为 pai_mcp_ 加 64 位十六进制随机值，数据库仅存 SHA-256 摘要。列表不返回明文或摘要。

每个用户将“仍有效的凭据”与“过去 24 小时内创建的凭据”合并计数，最多 20 条；撤销不会释放当天签发额度。签发通过用户行锁串行化，并在事务中重新检查和锁定原登录会话，避免密码重设后由旧会话签发新凭据；并发请求也不能突破限额。列表优先保留所有有效凭据，旧撤销记录仍在数据库中。

创建响应丢失时无法恢复明文；先列出并撤销未知凭据，再显式重新签发。接口不会自动重试创建。宿主名用于用户识别授权对象，不是设备身份认证；持有者仍可复制 token。

## MCP 接入与撤销

桥接从秘密环境变量 `MCP_ACCESS_TOKEN` 读取新凭据，以 Bearer 方式调用：

- GET /api/mcp/tools
- POST /api/mcp/tools/knowledge_search

这两个接口只接受专用凭据，拒绝 Cookie、混合 Cookie/Bearer、重复 Authorization、管理员 token 和网页登录 token。查询到凭据所有者后复用原知识检索执行器、服务端构造的用户上下文、每日 100 次工具额度和一次性 UUID 审计。所有宿主和普通工具接口共享用户额度；新增宿主或更换凭据不能重置额度、绕过 UUID 去重。scope 固定为 knowledge_search，不能扩权或选择其他工具。

每次请求都从数据库校验有效期、撤销状态和 scope，无进程缓存。撤销成功后，后续认证失败；已通过认证、正在执行的请求可能完成，撤销不是远程取消。重设密码在同一事务中撤销全部 MCP 凭据和网页登录会话；普通网页登出不撤销独立凭据。需要全面退出时逐一撤销或重设密码。

桥接仍仅连接本机 IPv4 回环 HTTP origin，默认关闭，启用需要 `MCP_ALLOW_EMBEDDING_COST=1`，每次工具调用也要求费用确认字段。宿主须遵守逐次用户确认约定；布尔字段不是真人授权证明。仍没有嵌入费用的精确金额上限。秘密不得写入仓库、日志、命令参数或普通客户端配置文件。

## 迁移与验证

新增迁移 `0023_mcp_credentials.sql`，创建带用户外键、固定 scope、到期时间和撤销时间的表。API 启动自动应用迁移。部署需先升级 API，再为每个宿主签发凭据、升级桥接并设置 MCP_ACCESS_TOKEN。旧 MCP_SESSION_TOKEN 不再生效；普通网页登录和 REST 工具接口保持原认证方式。

Rust 测试与 `make smoke-index` 覆盖持久化、到期、账户隔离、重复撤销、密码重设、并发额度、CSRF、拒绝扩权和 Cookie 降级，以及真实 stdio 检索与跨进程 UUID 去重。测试只使用本地向量夹具，无付费模型调用。

已提供[凭据管理与费用授权页面](sprint-3-mcp-credential-ui.md)，包含范围/期限/费用说明、一次性明文交付、列表与撤销。也可通过上述登录态 API 管理。
