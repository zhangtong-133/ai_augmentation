# Sprint 3：回复配置与金额账本运维

## 管理命令

新增 `reply-operations`，由管理员使用数据库凭据运行，只读取 `DATABASE_URL`。命令不加载供应商密钥、调用模型、启动 HTTP 或执行器，也不执行数据库迁移。数据库应先由正常 API 启动流程初始化。API 生产镜像包含此程序。

```bash
# 每页最多 100 个配置；有下一页时返回 next_cursor。
cargo run -p api-server --bin reply-operations -- configurations
cargo run -p api-server --bin reply-operations -- configuration "$REPLY_CONFIGURATION_REVISION"

# 默认仅预览。停用指定版本需要明确添加 --apply，重复执行保持同一停用时间。
cargo run -p api-server --bin reply-operations -- disable "$REPLY_CONFIGURATION_REVISION"
cargo run -p api-server --bin reply-operations -- disable "$REPLY_CONFIGURATION_REVISION" --apply

# 用实际用户 UUID、UTC 日期和币种核对金额账本。
cargo run -p api-server --bin reply-operations -- ledger \
  --user "$REPLY_AUDIT_USER_ID" --day 2026-09-30 --currency USD
```

部署栈中可使用 `./scripts/compose.sh exec -T api-server reply-operations`，后续参数与上述一致。查询和预览支持只读数据库账号：需对 `users`、`conversations`、`conversation_replies`、`reply_configurations`、`reply_money_daily`、`reply_money_reservations` 有 SELECT 权限；接入模型规划后，还需对 `model_planning_requests` 的 `user_id`、`conversation_id`、`request_id`、`status` 列授予 SELECT，不需读取冻结快照。接入第二阶段后，还需对 `model_execution_call_audit` 的相同四列授予 SELECT，不需读取阶段 JSON。配置查询只需对应配置表权限。执行停用另需对 `reply_configurations` 的 UPDATE 权限。命令没有重新启用、覆盖配置或修改金额端口。

配置结果包含版本、固定模型、币种、价格/计数版本、价格、token 上界、单次/日限额、创建/停用时间及数据库时钟下的有效性。失效或停用版本仍可查询。新价格或有效期继续使用新版本登记，沿用[不可变配置协议](sprint-3-reply-dispatch.md)。停用阻止新的预留和领取，不能撤回已发出的网络请求；queued 请求仍按已有取消协议退款。

## 日账本核对

`ledger` 必须显式指定用户、日期和币种，按同一用户 UTC 日核对所有配置的金额记录。无金额夹具请求不计入。明细包含请求/对话 ID、模型/配置版本、预留额、结算额、原因和时间，以及对话删除状态；不读取或返回消息、回复正文、标题、邮箱或密钥。对话及墓碑删除后仍保留金额凭据；用户删除继续级联清理。

接入[模型规划仓储](sprint-3-model-planning-store.md)后，同币种日账本包含付费回复和模型规划。明细新增 `request_kind`（`reply` / `model_planning`），`reply_status` 字段沿用原名并返回对应类型的请求状态；汇总覆盖两类独立凭据。

每次查询在 PostgreSQL `REPEATABLE READ, READ ONLY` 事务中读取日账本、全量汇总及当前页明细，单条语句时限 30 秒。使用数据库 numeric 聚合，累计预留与退款可以超过 i64；所有金额、价格和时间都输出精确十进制字符串。单位为微币种，USD 时 1 美元 = 1,000,000 微美元。

| 字段 | 含义 |
| --- | --- |
| `counts` | 全部请求、未结算、派发前取消、可信用量结算、保守保留及用量越界的次数 |
| `totals.reserved_micro` | 全部请求最初预留金额的累计值，包含之后退款的部分 |
| `totals.pending_micro` | 未结算请求当前保留的预留额 |
| `totals.settled_micro` | 已结算金额，包含可信用量及保守保留 |
| `totals.retained_micro` | 未知结果/失败等保留及用量越界保留的金额，是 settled 的一部分 |
| `totals.refunded_micro` | 已结算请求预留额减结算额的累计值 |
| `totals.expected_occupied_micro` | 未结算预留加已结算金额，即日账本应占用值 |
| `ledger_occupied_micro` | 日账本实际占用，缺失记录时为 null |
| `difference_micro` | 日账本实际占用减应占用；缺失记录以 0 计算差额 |
| `consistent` | 日账本与凭据一致；已有凭据但缺失日账本时，即使金额为 0 也返回 false |

有保守保留的项目需要人工对照供应商账单。这份报告记录本功能的内部金额占用，不代表供应商最终收款；覆盖模型 Agent 第二阶段 Embedding/回答凭据，不覆盖独立知识问答或文档索引的模型费用，不自动获取供应商账单，也不自动退款或修复差额。

明细按 `(conversation_id, request_id)` 分页，每页最多 100 项，`next_cursor` 可原样传给 `--after`。配置列表使用版本游标：`configurations --after REVISION`。账本每页的汇总均覆盖整个选定用户日，不是仅当前页。每次翻页会创建新快照；需要固定整份导出时，在暂停写入或数据库一致快照上执行。

## 退出状态与迁移

- 0：查询/预览成功、停用成功，或账本核对一致。
- 1：参数、权限、连接、记录不存在或存储错误；错误不输出数据库 URL、凭据或供应商响应。
- 2：账本存在差异或缺失必要日记录；stdout 仍提供完整 JSON 报告，数据保持原样。

新增 `0015_reply_audit_paging.sql`，只为用户/日期/币种及明细游标添加索引，不改动已应用迁移或现有预留/结算行为。

## 验证边界

新增真实 PostgreSQL 测试覆盖全部结算类型、用户/日期/币种隔离、删除对话及墓碑后的审计、并发结算快照、缺失/不一致账本、超过 i64 的汇总、205 条明细/配置分页及过期/停用状态。命令行集成测试使用只读数据库账号，验证查询不执行迁移、停用默认预览、权限拒绝、显式停用及重复停用、精确 JSON 和差额退出码 2。

`make test-postgres` 和 `make test-replies` 纳入这些测试；只读账号用例需要一次性测试数据库连接账号具有创建角色和授权能力，CI/smoke 的隔离 PostgreSQL 已满足。完整 smoke 还会在生产 API 容器中执行配置查询和按用户核对，验证程序已打包并可运行。自动测试不调用真实付费模型。

## 验收（2026-09-30）

- `make check`、前端 lint/typecheck/build、Compose 配置、脚本语法及差异检查通过。
- 完整 `make smoke` 通过：30 项 PostgreSQL（含新增 5 项）、8 项执行器、3 项付费 HTTP、2 项命令行权限/金额测试及 4 项 Redis 测试；生产容器内运维命令、双入口 HTTP、故障恢复和重启持久化通过。测试容器、网络和数据卷已清理。
- 本地本轮未重新运行 Playwright、MinIO/Qdrant 或公网抓取专项，没有页面变更，未调用真实付费模型。
