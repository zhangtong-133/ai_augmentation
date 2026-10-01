# Scheduler 运维：积压、租约和投递记录核对

新增 `scheduler-operations audit --user UUID [--after REQUEST_UUID]`。命令只连接已初始化 PostgreSQL，在同一个只读快照中核对该用户全部任务和提醒记录，并输出一页元数据。它不执行迁移、不启动 Scheduler、不读取模型配置或修改任务，不自动恢复、补发、取消或修复数据。本轮没有 SQL 迁移或新环境变量。

```sh
cargo run -p api-server --bin scheduler-operations -- audit --user "$SCHEDULER_AUDIT_USER_ID"
# 生产 API 镜像内同样提供此二进制。
./scripts/compose.sh exec -T api-server scheduler-operations audit --user "$SCHEDULER_AUDIT_USER_ID"
```

用户必须显式指定；不存在自动扫描所有用户的默认行为。不存在的用户返回错误，存在但没有任务的用户返回零计数和一致报告。未知、重复参数和变更类参数（如 `--apply`）均拒绝。

## 如何解读积压与租约

报告 `counts` 包含任务总数、有效草稿、过期草稿、待调度、运行中、已投递、已取消、失败、提醒记录及结构不一致任务数量。`due` 是 scheduled 中已经到期的子集，`expired_leases` 是 running 中租约过期的子集，不应把这些子集再次加到任务总数中。

- `ready_to_claim`：已经到期，且处于 scheduled 或 running 租约已过期的任务数。
- `oldest_ready_run_at_unix_ms`：上述任务中最早的原定执行时间，无积压时为 null。
- `oldest_ready_delay_ms`：快照时间减去上述执行时间；不是实际投递延迟，可能包含进程停机或其他等待时间。
- 明细中的 `ready_to_claim` 与同名全量计数使用相同条件。

过期租约是现有恢复协议允许的状态，不单独判为数据不一致。failed 表示执行器已停止该任务，但报告不推断失败的具体原因。积压数量和延迟也不代表正在发生数据库或进程故障。

`worker_liveness` 固定为 `unknown`：当前没有进程心跳，数据库不能证明后台在线、已经禁用或已退出。命令忽略自己的 `SCHEDULER_MODE` 和 `RUN_FOREVER`，不会把 API 容器的环境误当作 Scheduler 的有效配置。部署者应同时查看 `./scripts/compose.sh ps scheduler`、该服务的部署配置和脱敏日志；数据库中的积压只能作为排查线索。

## 结构核对与退出码

报告不读取标题、正文、授权 JSON、指纹或 `claim_id`，也不会把租约凭据输出给运维消费者。它检查状态、批准时间、租约截止时间与提醒是否存在，以及任务/提醒投递时间是否一致：

| issue | 含义 |
| --- | --- |
| missing_reminder | 任务已投递，但提醒记录缺失 |
| unexpected_reminder | 非已投递任务出现提醒记录 |
| delivery_time_mismatch | 任务和提醒的投递时间不同 |
| delivery_before_due | 记录的投递时间早于原定时间 |
| approval_outside_window | 批准时间不在原预览授权窗口内 |
| missing_approval_time | 待调度、运行中或已投递任务没有批准时间 |
| unexpected_lease | 草稿、待调度、取消或失败任务残留租约截止时间 |

一个任务可以有多个 issue，`counts.inconsistent` 按任务计数。`consistent=true` 仅表示本次元数据结构核对没有问题，不证明正文、授权指纹有效，也不代表供应商、用户设备或调度进程健康；执行器领取和投递前仍独立执行完整授权校验。

退出码 0 表示核对完成且结构一致（可以存在积压、过期租约或 failed）；1 表示参数、权限、连接、用户不存在或存储错误；2 表示结构不一致，stdout 仍输出 JSON。告警系统应分别根据 `consistent`、积压和失败数量设置业务阈值，不应把退出码 0 等同于无待处理任务。

## 快照、分页和权限

每次调用使用 `REPEATABLE READ, READ ONLY` 事务和一个固定数据库时间，语句超时 30 秒。全量汇总、分页明细和所有时间分类均来自同一快照，避免并发投递导致虚假的“缺失提醒”。明细按请求 UUID 排序，每页最多 100 条，`next_cursor` 可原样用于 `--after`。即使不一致记录在后续页，第一页仍返回全量不一致计数及退出码 2。翻页使用新快照，需要完全一致的多页导出时应在停止相关写入的窗口执行。

时间和延迟输出为精确十进制字符串，缺失时间为 null。计数保持 JSON 整数，受现有每用户保留上限约束。明细包含协议版本和状态，不包含可重新派发任务的凭据。

查询账号仅需以下列权限；可同时设置 `default_transaction_read_only=on`。其中 `audit_reader` 是部署者自行创建的角色名：

```sql
GRANT SELECT (id) ON users TO audit_reader;
GRANT SELECT (user_id,request_id,version,status,run_at_ms,created_ms,
  approval_expires_ms,approved_ms,lease_until_ms,delivered_ms,cancelled_ms)
  ON schedules TO audit_reader;
GRANT SELECT (user_id,request_id,delivered_ms) ON schedule_reminders TO audit_reader;
```

不需要标题、正文、授权、指纹或租约令牌列权限，不需要 UPDATE、INSERT 或迁移权限。凭据通过数据库连接环境提供，错误不输出连接 URL 或数据库内部异常。

## 验证

真实 PostgreSQL CLI 测试覆盖仅元数据列可读的只读角色、全部任务分类和积压、空用户、跨用户隔离、未知用户、缺失/意外/时间不一致提醒及批准窗口异常、错误退出码与数据不变、105 条分页及全量异常计数、大整数精度、并发原子投递过程的快照一致性。`make test-replies` 和完整 smoke 已纳入测试；smoke 也验证生产镜像内命令可运行。

## 验收（2026-10-01）

- `make check`、前端 lint/typecheck/build、Compose 配置、脚本语法和差异检查通过。
- 完整 `make smoke` 通过：89 项 PostgreSQL、1 项 Scheduler 进程、8 项执行器、8 项付费/模型/Scheduler HTTP、9 项运维命令（含新增 4 项 Scheduler）、2 项固定计划 HTTP 及 4 项 Redis；生产容器内新命令、双入口 HTTP、重启持久化和缓存故障恢复通过。测试容器、网络和数据卷已清理。
- 本轮无页面变更，未重跑 Playwright、MinIO/Qdrant 或公网抓取专项，未调用真实付费模型或外部通知。
