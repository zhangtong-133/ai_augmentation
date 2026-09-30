# Sprint 3：工具调用次数预算与持久化审计

## 共用执行入口

`agent-core::tool_execution::AuditedToolExecutor` 包装现有固定白名单执行器：纯参数预检 → PostgreSQL 一次性调用/次数登记 → 工具执行 → 终态元数据。应用提供已认证的身份，工具继续负责数据归属校验。现有 `knowledge_search` HTTP 接口已接入此入口，后续受限 Agent 计划可复用。

本步没有模型自动规划、工具循环、MCP 传输或 Scheduler，也没有新的页面。知识检索仍由用户显式调用，开启 `KNOWLEDGE_INDEX_ENABLED=true` 才注册。

## HTTP 协议

`POST /api/tools/knowledge_search` 保持参数正文 `{"query":"问题","limit":5}`，现在必须带一个 `Idempotency-Key: UUID` 请求头。调用方在发送前生成并保留 UUID；缺失、无效或重复请求头返回 400 `invalid_tool_call_id`。Cookie 会话、CSRF 和白名单要求保持一致。成功响应在原有 `tool`/`output` 外增加 `call` 元数据：

```json
{
  "request_id": "客户端生成的 UUID",
  "tool": "knowledge_search",
  "day": "2026-09-30",
  "status": "succeeded",
  "input_bytes": 28,
  "output_bytes": 11,
  "created_at_unix_ms": 1790733600000,
  "deadline_unix_ms": 1790733660000,
  "finished_at_unix_ms": 1790733600100
}
```

`GET /api/tools` 在每个工具描述中增加 `request_id_header` 和 `daily_call_limit`。未启用工具时仍返回空列表。现有调用方需要增加请求头；参数 schema 不增加用户、对话或审计字段。Next.js 仅在该工具 POST 路径转发请求头，放行只读审计路径；Nginx 继续原样代理。

同一用户的 ID 在跨日期、进程和重启后保持一次性：相同工具及参数返回 409 `tool_call_already_used`，不同工具/参数返回 409 `tool_call_conflict`。参数指纹使用排序后的 JSON，忽略键顺序和空白；默认值省略与显式提供仍属于不同参数。返回 409 时不再次调用模型或占用次数。

响应丢失后可以查询：

- `GET /api/tool-calls/{request_id}`：仅查当前用户的执行记录，其他用户或不存在的 ID 返回 404。
- `GET /api/tool-calls`：同一只读快照返回数据库 UTC 今天的 `day`、`used`、`limit` 和全部当日记录，最多 100 条。
- `GET /api/tool-calls?day=2026-09-30`：查询指定 UTC 日，严格验证公历日期；拒绝额外参数和客户端用户 ID。

成功查询与调用响应使用 `Cache-Control: no-store`。工具停用后历史仍可查询。审计不保存或重放结果正文；需要再次检索时，调用方必须显式生成新 ID，重新占用次数。

## 次数与故障语义

每用户 UTC 日最多 **100 次执行尝试**，所有注册工具共享。日范围取登记时的数据库时钟；用户锁串行化查重和次数增长，调用记录与次数在同一事务提交。无效参数、未注册工具、认证/CSRF 拒绝不占用次数。达到上限返回 429 `tool_daily_limit`。

这份预算限制调用次数，不是货币预算，也不代表实际 Embedding 请求次数或供应商费用。检索仍可能计费；它与付费回复金额账本、普通检索/问答接口的额度独立。

登记成功后，失败、依赖繁忙、超时、输出超限和取消都保留一次尝试。没有自动重试或退款。登记失败返回 503 并阻止工具执行；工具返回后写终态失败也返回 503，已登记记录保留，原 ID 不能再次执行。

执行器仍限制全局并发 2、单次 35 秒、输入 8 KiB、输出 64 KiB。记录有 60 秒审计期限。崩溃或请求取消留下 `running`；超过期限后查询显示 `unknown`，底层一次性登记与次数继续保留，不领取或重派。异步取消不能撤回已发送的供应商请求。可靠终态晚到时可以补齐原记录，但不会增加或退还次数。

终态是 `succeeded`、`failed`、`timed_out`、`busy`、`denied` 或 `output_rejected`。相同终态/输出大小重复保存保持同一完成时间；拒绝覆盖其他终态。失败不会伪装成空检索结果，也不返回供应商或数据库异常正文。

## 存储与边界

新增迁移 `0016_tool_calls.sql`：`tool_daily_budgets` 保存用户 UTC 日次数，`tool_calls` 保存 ID、工具名、内部参数指纹、大小、状态和时间。没有参数、查询、正文、片段、邮箱或密钥列；指纹不由 API 返回。记录按用户保留，用户删除级联清理。

只读查询使用 `REPEATABLE READ, READ ONLY` 和 30 秒语句时限。工具执行不会持有数据库事务或用户锁。API 身份只取自 Cookie 会话；独立工具上下文 ID 仍由服务端生成，不代表持久化对话。未来 Agent 编排接入真实对话时，还需验证对话归属、用户授权和每个计划的调用上限。

## 验证

- `make check` 覆盖执行前拒绝、参数指纹、一次性执行、工具错误/超时/输出限制、取消和终态写入失败，以及 HTTP 请求头、会话、CSRF、错误脱敏与审计隔离。
- `TEST_DATABASE_URL=… make test-tools` 使用一次性 PostgreSQL，验证并发查重、最后一次额度竞争、重连持久化、结果未知、一次性终态、日/用户隔离、非法输入回滚、一致快照及用户删除级联。
- `make smoke-index` 使用真实 PostgreSQL/Qdrant 与本地模型夹具，通过 Next.js/Nginx 双入口验证工具结果、请求头、重放拒绝、元数据审计、用户隔离和 API/数据库重启后的记录。
- 本步不调用真实付费模型；公网抓取和新 UI 不属于本步验收。

## 验收（2026-09-30）

- `make check`、前端 lint/typecheck/build 通过，新增 4 项执行入口单测、3 项 HTTP 单测及 4 项真实 PostgreSQL 测试。
- 完整 `make smoke-index` 通过：34 项 PostgreSQL、8 项回复执行器、3 项付费 HTTP、2 项管理员命令、4 项 Redis 和 1 项 Qdrant 测试；双入口工具审计、查重及重启持久化验收通过，测试资源已清理。
- 本地本轮未重跑 Playwright、MinIO 或公网抓取专项，未调用真实付费模型。没有页面变化。
