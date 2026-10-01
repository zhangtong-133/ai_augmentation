# 模型 Agent 配置与金额账本运维

模型助手新增 `model-agent-operations` 管理命令，连接已有 PostgreSQL；不执行迁移，不读取供应商密钥，不启动模型、索引器或后台任务。默认输出 JSON，只有 `disable ... --apply` 写入停用时间。本轮无 SQL 迁移。

## 配置查询与停用

```sh
cargo run -p api-server --bin model-agent-operations -- configurations planning
cargo run -p api-server --bin model-agent-operations -- configurations execution
cargo run -p api-server --bin model-agent-operations -- configuration planning "${MODEL_AGENT_CONFIGURATION_VERSION}-planning"
cargo run -p api-server --bin model-agent-operations -- configuration execution "${MODEL_AGENT_CONFIGURATION_VERSION}-answer"

# 默认预览；添加 --apply 才持久化停用。两个阶段分别停用。
cargo run -p api-server --bin model-agent-operations -- disable planning "${MODEL_AGENT_CONFIGURATION_VERSION}-planning" --apply
cargo run -p api-server --bin model-agent-operations -- disable execution "${MODEL_AGENT_CONFIGURATION_VERSION}-answer" --apply
```

生产容器同样提供该二进制，可用 `./scripts/compose.sh exec -T api-server model-agent-operations` 加上述参数调用。`STAGE` 必须为 `planning` 或 `execution`，版本取自配置列表，不能以部署根版本代替完整版本。执行配置包含 Embedding 和回答两个预算，整体停用；不提供单独恢复或覆盖配置入口。停用两个阶段是两次独立操作，无跨阶段原子性。

配置列表按版本排序，每页 100 条，将 `next_cursor` 原样传给 `configurations STAGE --after VERSION`。详情包含阶段、配置版本、固定模型、价格/计数版本、token 上界、金额及日次数上限。配置中的所有数字（含时间和次数）输出为十进制字符串，避免客户端舍入。执行阶段有效期取两个模型预算的较早期限，`active` 由数据库语句时钟和停用记录决定；已过期或停用的配置仍可查看。

默认 `disable` 仅查询现状并输出 `apply:false`。显式停用幂等，重复调用保留最初停用时间。停用阻止该版本后续预览、批准和领取；执行器发送前仍复查配置。已经发出的请求无法撤回。停用不自动取消现有请求、结算或退款，待执行请求继续按既有取消/过期协议处理。更新价格或有效期必须登记新版本。

查询/预览仅需相应 `model_planning_configurations` 或 `model_execution_configurations` 表的 SELECT 权限。持久化停用另需该表 `disabled_at` 列的 UPDATE 权限；只读账号使用 `--apply` 会失败。错误不输出连接 URL、密钥或数据库内部细节。

## 共享金额账本核对

```sh
cargo run -p api-server --bin model-agent-operations -- ledger \
  --user "$MODEL_AUDIT_USER_ID" --day 2026-10-01 --currency USD
```

复用 [reply-operations 金额核对协议](sprint-3-reply-operations.md)，包括只读账号授权、快照一致性、分页、精确金额和退出码。选定用户、UTC 日和币种后，核对共享日账本内的全部凭据：`reply`、`model_planning`、`model_execution`。不能只汇总模型 Agent 凭据后与共享总占用比较。第二阶段每次 Embedding 和回答各有独立凭据，不再累加父阶段报价，以免重复计费。该命令核对金额，不核对模型/工具日次数，也不查询供应商最终账单。

明细不输出私有消息、搜索查询、证据或回答。删除对话后金额凭据仍保留；用户删除按既有级联清理。请求状态沿用 `reply_status` 字段；已经清理的规划请求可能没有状态。总计始终覆盖选定用户的整日，`--after CONVERSATION_UUID/REQUEST_UUID` 仅影响当前明细页。

退出码 0 表示操作成功或账本一致；1 表示参数、权限或存储错误；2 表示差额/缺失必要账本记录，stdout 仍输出报告。差额报告不会自动修复数据。

## 验证

命令行真实 PostgreSQL 测试覆盖两个阶段只读查询、显式停用/权限拒绝、重复停用、阶段隔离、执行配置最早过期时间、跨页连续性、超过 JavaScript 安全整数的金额及差额退出码。`make test-replies` 纳入这些测试，`make smoke` 同时验证生产容器打包及查询。自动测试不调用付费模型。

## 验收（2026-10-01）

- `make check`（rustfmt、Clippy、工作区测试）、前端 lint/typecheck/build、Compose 配置和脚本语法检查通过。
- 完整 `make smoke` 通过：79 项 PostgreSQL、8 项执行器、6 项付费/模型 HTTP、2 项回复运维及 3 项模型运维、2 项固定计划 HTTP 和 4 项 Redis 测试；生产容器内新增命令、双入口 HTTP、重启持久化和缓存故障恢复通过。测试容器、网络及数据卷已清理。
- 本轮无页面变更，未重新运行 Playwright、MinIO/Qdrant、公网抓取专项，未调用真实付费模型。
