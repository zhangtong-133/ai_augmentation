# Sprint 3：受限 Agent 计划与用户授权

## 本次交付

提供固定版本 `knowledge-search-v1` 的只读检索编排：用户提交 1–3 个查询，先读取持久化计划，再确认指纹、次数上限及向量模型费用，服务端逐步执行并保存结果。查询和顺序由用户明确提供，每步只调用 `knowledge_search`；此版本没有自然语言模型规划、答案生成、记忆写入或动态工具选择。

计划依附已有、未删除的用户对话及消息版本，并保存固定计划版本；不支持的版本不能授权或领取。创建和授权均不调用模型；只有授权成功后的首次执行会调用现有 Embedding 适配器。沿用 `KNOWLEDGE_INDEX_ENABLED`，不增加运行时环境变量。索引关闭时仍可读取历史计划，创建和授权无法通过工具预检。

## HTTP 协议

所有接口需要 Cookie 会话，写操作还需要 `X-Requested-With: personal-ai`。管理员 Bearer token 不授予用户权限；身份只取自会话。响应带 `Cache-Control: no-store`。

| 方法与路径 | 行为 |
|---|---|
| `POST /api/conversations/{id}/agent-plans` | 保存预览，返回 200；同 ID、相同定义重放返回已有计划 |
| `GET /api/conversations/{id}/agent-plans` | 返回 `enabled`、`version`、`max_tool_calls` 和最多 20 个 `plans` |
| `GET /api/conversations/{id}/agent-plans/{request}` | 查询授权、步骤状态与已保存结果 |
| `POST /api/conversations/{id}/agent-plans/{request}/approve` | 精确授权，返回 202；只有首次授权启动进程内执行器 |
| `POST /api/conversations/{id}/agent-plans/{request}/cancel` | 幂等取消未完成计划，返回 200 |

创建示例：

```json
{
  "request_id": "客户端生成并保留的 UUID",
  "expected_revision": 1,
  "searches": [
    {"query": "第一项问题", "limit": 5},
    {"query": "第二项问题", "limit": 3}
  ]
}
```

`expected_revision` 必须等于当前消息版本且为 1–100。每计划 1–3 个互不重复的查询；query 为 1–1000 个字符、不得全空白或含 NUL；limit 为 1–5，默认 5。去除首尾空白后相同的查询算重复。拒绝额外字段及客户端用户 ID、工具名称、输出或授权内容，结构错误返回 422、取值无效返回 400。同一对话内的请求 ID 更改定义返回 409。每对话最多保存 20 个计划，每用户最多 100 个，超限返回 429；删除对话可释放计划存储容量。

预览返回 `request_id`、`conversation_id`、`revision`、`version`、`digest`、`status`、`tool_call_limit`、`attempted`、时间及 `steps`。每步包含固定工具名、完整 `arguments`、服务端一次性 `call_id`、状态和可空 `output`。`digest` 是 SHA-256，绑定版本、用户、对话、请求 ID、消息版本、全部查询、顺序和调用上限。

授权正文必须包含：

```json
{
  "plan_digest": "预览返回的 digest",
  "accepted_call_limit": 2,
  "acknowledge_embedding_cost": true
}
```

指纹和次数须与预览完全一致；不确认费用返回 400，指纹/次数不符或计划失效返回 409。重复授权返回现有状态，不启动第二个执行器、不重置期限。授权响应可能仍为 `running`，调用方通过详情查询观察终态。新 ID 需要重新预览和授权，可能再次计费。

Next.js 固定放行这些方法和路径，复用 CSRF、16 KiB 请求体限制及 10 秒代理等待；授权后异步执行，所以 HTTP 无需等待三个工具调用。Nginx 使用已有 API 代理。

## 执行与状态

| 计划状态 | 含义 |
|---|---|
| `draft` | 未授权，不执行、不占工具次数；创建后 15 分钟内可授权 |
| `running` | 已授权；授权后有 150 秒完成期限 |
| `succeeded` | 全部步骤成功，结果持久化 |
| `failed` | 工具失败或日次数已用尽，停止后续步骤 |
| `stale` | 消息版本改变或领取时计划版本不受支持，需要创建新计划 |
| `cancelled` | 用户取消；清除已保存输出，阻止后续领取 |
| `expired` | 草稿超过授权期限，读时显示此状态 |
| `unknown` | 执行或审计结果未能确认，超过期限；禁止重新领取 |

步骤按顺序执行，每步领取事务依次锁定用户和所属对话，复核授权、版本及每计划次数。在同一事务登记已有工具日预算/审计、递增 `attempted` 并标记 `dispatching`；事务提交成功后才调用工具。其他执行器看到正在派发的步骤就停止，不并发领取下一步。外部调用期间不持有数据库事务。

每个步骤的 ID 与独立工具接口共用去重账本；从其他入口重放也不能再次执行。所有计划和独立工具调用共享用户 UTC 日 100 次尝试上限。失败、超时、并发忙和结果未知都保留已登记次数；未领取步骤不占次数。达到日上限时计划进入 `failed`，可在工具审计中核对用量。

结果保存前先写工具审计终态。审计存储失败或计划保存失败时停止，不重新调用工具；已经登记的调用会在 60 秒期限后显示结果未知。成功结果是经 PostgreSQL 归属复核的知识片段，并继续视为不可信资料，不用于执行代码或改写权限。底层依赖错误正文不会保存到计划，也不会返回给客户端。

取消与领取共用锁：取消先提交则无法领取；领取先提交的调用视为已经派发，可能已发送至供应商，不能撤回或退还次数。晚到结果只补齐独立工具元数据，取消后的计划不保存正文。取消终态计划返回原状态。删除对话立即级联清除计划查询和结果，阻止后续步骤；工具元数据及已用次数独立保留。

## 重启与授权边界

执行器仅由首次授权触发，不扫描历史计划，不自动恢复或重试未知结果。进程在授权后、派发前退出时，该计划可能没有工具记录，但仍不会因原授权重放而启动；150 秒后查询显示 `unknown`。在派发后退出时，工具记录和次数保留，60 秒后显示 `unknown`。使用新计划须由用户明确再授权。

授权是本次计划的持久化许可；退出登录不会自动撤销已经授权的步骤。取消计划或删除对话可阻止尚未派发的步骤。新增工具、模型循环、自动事实提取、定时运行及 MCP 传输需单独实现其授权和费用边界。

## 存储与验证

新增迁移 `0017_agent_plans.sql` 和 `0018_agent_plan_versions.sql`：`agent_plans` 保存不可变定义指纹、授权、版本、次数与期限，`agent_plan_steps` 保存有界查询及结果。计划版本持久化，部署升级后也需匹配固定版本才能执行。用户删除级联清理，对话删除立即清除计划，独立 `tool_calls` 的元数据继续保留。

- `make check`：参数、重复查询、授权指纹、HTTP 登录/CSRF 和非法身份字段。
- `TEST_DATABASE_URL=… make test-agent`：真实 PostgreSQL，并发授权/领取、计划版本保护、三次上限、日额度竞争、消息版本变化、取消/删除、到期/重连及 COMMIT 失败阻止外部调用；HTTP 使用本地 Embedding 夹具验证完整执行及失败后停止。
- `make smoke-index`：真实 PostgreSQL/Qdrant、本地模型夹具，Next.js/Nginx 双入口预览、授权、执行、审计、取消、用户隔离及 API/数据库重启后的查重。
- 前端 lint/typecheck/build 验证代理；后续已提供[计划预览与授权页面](sprint-3-agent-plan-ui.md)。

### 验收记录（2026-09-30，macOS / OrbStack）

`make check`、前端 lint/typecheck/build、`make compose-config`、smoke 脚本语法及差异检查通过。最终完整 `make smoke-index` 通过 44 项 PostgreSQL 集成测试（含 10 项计划测试）、2 项 Agent HTTP 执行测试，并通过回复、Redis、Qdrant 回归以及计划双入口和重启查重。取消、删除、版本变化、未知结果和 COMMIT 失败均有分层覆盖。测试容器、网络和数据卷已清理，构建镜像保留。

本次 API 交付未运行本地 Playwright、MinIO 专项、公网网页成功抓取或真实付费模型；页面和浏览器验收另见[计划页面记录](sprint-3-agent-plan-ui.md)。
