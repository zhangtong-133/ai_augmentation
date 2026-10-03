# Sprint 4：学习数据只读运维核对与额度诊断

新增 `LearningOperationsStore` 和 `learning-operations` 管理员命令，查询指定用户的学习元数据。使用 `connect_existing`，不执行迁移、不访问模型或对象存储、不创建计划、不修改或自动修复记录。无新增迁移和应用环境开关，生产 API 镜像包含此命令。

模型授权、一次性派发与发送审计另见 [订阅核验运维](learning-model-operations.md)，通过独立 `audit-models` 子命令和列权限查询。

## 使用

使用已初始化数据库的只读连接配置 `DATABASE_URL`，运行：

```sh
cargo run -p api-server --bin learning-operations -- audit --user USER_UUID
cargo run -p api-server --bin learning-operations -- audit --user USER_UUID --after PLAN_UUID
learning-operations --help
```

必须显式指定单个非零用户 UUID；游标也必须非零 UUID。重复参数、未知参数或写操作参数在连接数据库前被拒绝。成功查询输出一份 JSON：退出码 0 表示所列元数据检查没有发现异常，2 表示发现不一致或超限，1 表示参数、用户、数据库或超时错误。错误不输出连接字符串和私有内容。

## 输出

- `revision`、`snapshot_at_unix_ms` 及计划的版本/时间均为十进制字符串，避免 JavaScript 整数精度损失。
- `counts` 汇总技能、启用/删除技能、边、自评、历史版本自评、计划各状态、历史快照计划、任务、待记录任务和完成/取消结果。计数覆盖当前用户全量元数据，不受分页游标影响。
- `quotas` 给出技能 100、自评 1000、计划总数 1000、UTC 当日计划 10 的 used/limit/remaining。计数包括删除墓碑、失效计划和分数已清除的自评，与写入口一致；当日边界是数据库时间所在 UTC 日的 00:00，未来记录也进入当日计数并被另行标为异常。剩余额度最低为 0。
- 恰好达到上限只产生 `*_quota_exhausted` 提示，不构成不一致；超过上限产生 `*_limit_exceeded` 问题代码和退出码 2。
- `items` 仅包含计划 UUID、状态、快照版本、创建时间和问题代码。每页最多 100 条，按 UUID 升序，用 `next_cursor` 翻页。没有明细的尾页仍返回完整计数和全局异常。

## 检查范围

图检查使用去重的可达节点对判断环，正常上限内最多 10000 对，避免枚举所有路径造成指数级遍历。另检查超过 8 项先修、已删除技能仍有出边、边引用缺失节点。不可用的前置技能单独计数，不视为损坏：删除技能后依赖保留并阻塞是正常行为。技能数超过 100 时直接报告额度异常，并给出 `cycle_check_skipped_over_limit`，不继续无界图遍历。

版本检查包括：当前学习版本不低于“现存技能版本总和 + 自评条数”这个已发生变更的下界，自评引用的技能版本不超前、自评的预期快照必须早于当前状态、自评时间不在未来。历史版本自评是正常保留记录，只计数，不把它推断为当前能力。

计划检查包括：未来生成时间、快照版本超前、任务数量超过 5 或序号不连续、任务状态与计划状态不匹配、ready 计划缺少来源、deleted 计划仍有来源、来源数量超过 100、非活动任务残留训练结果、结果时间早于计划或晚于诊断时钟。

新计划可以合法包含生成前已有的技能墓碑，因此“ready 计划存在 deleted 来源”本身不能证明清理遗漏。命令不读取原计划中的技能版本，不对此作武断判定。

`consistent` 只表示上述元数据检查通过。它不读取技能名称、自评分数、计划/任务 JSON、摘要或结果备注；不重新计算计划摘要，不逐项比较 JSON 与任务定义，不验证正文已清空，也不能区分合法空任务计划和任务整批丢失。正文和摘要验证继续由现有学习仓储读取路径负责。额度剩余不保证下一次操作成功，实际写入仍需版本、图合法性、任务归属及幂等校验。

## 只读快照与最小权限

汇总、异常检测和当前页使用同一个 `REPEATABLE READ, READ ONLY` 事务。先查询用户建立快照，再采样数据库时钟；每条语句设 30 秒超时，超时返回错误而不是部分健康报告。并发写入不会混入当前报告；不同分页调用使用各自快照，不承诺跨页冻结。

可为已有运维角色授予下列列权限。账号创建、连接权限和密码由部署者管理；`--user` 是查询范围，不是数据库级用户隔离授权，拥有这些列权限的管理员可以选择其他用户。

```sql
GRANT USAGE ON SCHEMA public TO learning_reader;
GRANT SELECT (id) ON users TO learning_reader;
GRANT SELECT (user_id,revision) ON learning_state TO learning_reader;
GRANT SELECT (user_id,id,revision,enabled,deleted) ON learning_skills TO learning_reader;
GRANT SELECT (user_id,skill_id,prerequisite_id) ON learning_edges TO learning_reader;
GRANT SELECT (user_id,skill_id,skill_revision,expected_revision,assessed_ms)
  ON learning_assessments TO learning_reader;
GRANT SELECT (user_id,request_id,snapshot_revision,created_ms,status)
  ON learning_plans TO learning_reader;
GRANT SELECT (user_id,request_id,skill_id) ON learning_plan_sources TO learning_reader;
GRANT SELECT (user_id,request_id,id,ordinal,status) ON learning_tasks TO learning_reader;
GRANT SELECT (user_id,task_id,outcome,recorded_ms) ON learning_results TO learning_reader;
```

不需要任何写权限、迁移表权限或私有正文列权限。输出不包含姓名、技能名称、分数、训练备注、请求摘要或任务正文。异常需要管理员结合业务上下文另行排查，命令不提供 `--apply`。

## 验证

`make test-learning-operations` 已接入 CI 和隔离 smoke：测试最小列权限、私有列/写/迁移访问被拒绝、退出码、分页及账户范围、诊断前后数据不变、精确大版本、图环与墓碑、结果/来源状态异常、未来时间、配额耗尽与超限、清除记录计入额度、尾页仍反映全局异常，以及并发删除下的一致快照。smoke 还在生产镜像内执行命令并检查空账户报告。

本步不改 HTTP 或页面；浏览器、对象存储和索引专项不属于此次本地重跑范围。

本轮验证通过：Rust 1.98 全仓 `make check`、前端 lint/typecheck/build、129 项 PostgreSQL 仓储回归、7 项学习运维数据库测试（含 1 项补充测试单独运行），以及完整 `make smoke` 的 Rust 1.96 生产镜像、镜像内诊断和双入口 HTTP 验收。隔离测试资源已清理。

## 权限夹具的 CI 并发修复

1acfa90 的 CI 在两个权限夹具并行 `GRANT` 时出现 PostgreSQL `XX000: tuple concurrently updated`（原计划权限测试的角色授权语句）。不同角色的授权仍会更新相同 schema/table ACL 目录元组。本轮以共享异步互斥锁仅串行化两个权限夹具的完整角色生命周期，包括 CREATE/GRANT、只读验证和 DROP OWNED；其他业务并发、快照和隔离测试继续并行。该锁只在此测试进程内生效，不是生产锁，也不替代多个外部测试进程共享数据库时的隔离。

修复后，使用本轮一次性数据库以 `--test-threads=16` 连续运行 10 轮完整学习运维套件，每轮 10 项均通过；Clippy 复验通过。不需要更换对象存储或修改生产数据库权限。
