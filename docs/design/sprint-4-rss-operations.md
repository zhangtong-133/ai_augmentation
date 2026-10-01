# Sprint 4：RSS 只读运维核对与状态诊断

## 使用

新增管理员命令 `feed-operations`，已打包到 API 生产镜像。仅连接现有数据库，不执行迁移，不读取 RSS 部署配置，不请求来源，不领取、取消、恢复或重派采集。

```bash
# DATABASE_URL 指向要核对的数据库，可使用仅有下述列权限的账户。
cargo run -p api-server --bin feed-operations -- audit --user USER_UUID
cargo run -p api-server --bin feed-operations -- audit --user USER_UUID --after REQUEST_UUID
# 已启动的 Compose 部署：
docker compose exec api-server feed-operations audit --user USER_UUID
```

`--user` 必填；不提供全用户扫描、修复或 apply 参数。输出单个 JSON，错误仅打印脱敏信息。退出码 0 表示已完成元数据核对且未发现一致性问题，2 表示发现不一致，1 表示参数、用户或数据库错误。超时/未知结果属于待处理提示，不单独导致退出码 2。

## 输出与核对边界

`counts` 是当前用户全量汇总，与 `--after` 无关：

- 订阅总量（含删除墓碑）、启用数、删除数；条目数量、删除订阅残留条目数和时间异常条目数。
- 采集总数及 draft/running/succeeded/failed/unknown/cancelled 各状态数。
- `previews_last_24h` 使用滚动 24 小时窗口，`claimed_today` 使用 UTC 自然日，与写入口计数边界一致；剩余预览和采集额度分别按 100 和 20 扣减，最低为 0。
- `expired_drafts` 按当前固定五分钟窗口计算；`overdue_running` 根据保存的 60 秒执行截止时间计算。
- `inconsistent_collections` 汇总时间倒置、未来时间或审计缺失/不匹配的请求。

请求明细只包含请求/订阅 ID、状态、固定失败原因、创建与截止毫秒时间、问题代码。时间使用十进制字符串；每页最多 100 条，按 UUID 排序，以 `next_cursor` 翻页。每次调用建立独立快照，因此跨页调用不保证历史完全静止。

审计核对要求 draft 事件与创建时间匹配，有领取时 running 事件与领取时间匹配，终态事件的时间、状态、原因及新增/更新/未变化数量全部匹配；还核对事件总数以发现额外事件。订阅/并发/日额度超过仓储上限也会产生问题代码。

`consistent` 只表示这些元数据检查通过。命令不读取或重算保存计划、同意摘要、执行凭据、条目内容摘要，不验证来源是否可达、进程是否存活，也不把过期草稿、普通采集失败或未知结果直接判作数据损坏。诊断不会修改任何记录；超时请求需用户在现有页面核对后显式恢复，unknown 不应自动重新采集。额度剩余不保证可以立即执行，订阅版本、计划期限、并发及服务开关仍由实际执行入口检查。

## 快照与权限

汇总、问题检测及当前页共用 `REPEATABLE READ, READ ONLY` 事务。先建立数据库快照，再采样数据库时钟，避免快照中已提交记录被采样顺序误判为未来记录。每条语句限时 30 秒，超时返回错误；不把不完整结果作为健康报告。

可为已有运维角色授予以下最小列权限；角色创建、登录凭据及数据库连接权限由部署者管理。`--user` 是查询范围，不是数据库角色的用户级访问控制；持有这些列权限的运维人员可以选择不同用户。

```sql
GRANT USAGE ON SCHEMA public TO rss_reader;
GRANT SELECT (id) ON users TO rss_reader;
GRANT SELECT (user_id,id,enabled,deleted) ON feed_subscriptions TO rss_reader;
GRANT SELECT (user_id,request_id,subscription_id,status,created_ms,claimed_ms,
  deadline_ms,finished_ms,reason,inserted,updated,unchanged)
  ON feed_collections TO rss_reader;
GRANT SELECT (user_id,subscription_id,first_seen_ms,updated_ms,last_seen_ms)
  ON feed_entries TO rss_reader;
GRANT SELECT ON feed_collection_audit TO rss_reader;
```

不需要订阅名称、来源 URL、plan、digest、accepted_digest、claim_id、条目标题/摘要/链接等列的权限。输出也不包含这些字段。运行命令不需要 RSS 公网开关或模型凭据。

## 验证与后续

PostgreSQL 测试覆盖用户隔离、超时诊断不恢复、正常终态、审计缺失/计数不匹配、删除残留、条目时间异常以及 101 条分页和全量汇总。CLI 集成测试创建只具有元数据列读取权限的临时角色，验证运行成功、私有列/写操作被拒、未知用户与无效参数失败及不一致退出码 2。`make test-feed-operations` 已纳入 CI 和隔离冒烟，冒烟同时验证镜像中的命令可执行。

后续进入 RSS 价值评分与 Daily Brief 的规划；自动轮询和自动重试仍未实现。

本步验证通过：Rust 1.98 全仓 `make check`、前端 lint/typecheck/build、106 项 PostgreSQL 仓储回归、最小列权限 CLI 集成测试，以及完整 `make smoke`（包含 Rust 1.96 生产构建、镜像内 RSS 命令和双入口 HTTP）。未改动页面，未重跑浏览器、对象存储或索引专项；未请求真实 RSS 或付费模型。隔离测试数据库、容器和网络均已清理。
