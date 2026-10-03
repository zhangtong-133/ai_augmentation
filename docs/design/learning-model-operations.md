# 订阅核验只读运维与学习闭环验收

为[一次性模型核验](learning-model-execution.md)增加 `learning-operations audit-models`。不新增迁移、HTTP 接口或模型调用；已有 `audit` 及其最小权限保持兼容。

## 查询与诊断

对已初始化的数据库设置只读 `DATABASE_URL` 后运行：

```sh
cargo run -p api-server --bin learning-operations -- audit-models --user USER_UUID
cargo run -p api-server --bin learning-operations -- audit-models --user USER_UUID --after REQUEST_UUID
```

必须显式指定非零用户 UUID，游标同样必须非零 UUID。输出 JSON，退出码 0 表示本次元数据检查未发现不一致，2 表示异常或超限，1 表示参数、数据库、用户不存在或查询超时错误。不会运行迁移、清理超时请求、修复数据、读取本机凭据或重发模型请求。

每次调用在一个 `REPEATABLE READ, READ ONLY` 事务中读取。先通过用户查询建立快照，再采样数据库时钟；每条查询 30 秒超时。计数、全局问题与当前页使用同一快照，不同分页调用不冻结跨页数据。每页至多 100 条，按请求 UUID 升序，通过 `next_cursor` 继续。尾页即使为空也包含全局计数与问题。

报告只包含用户/请求/来源/连接 UUID、状态、连接版本、生命周期时间和问题代码。版本和时间使用十进制字符串。计数覆盖八种状态及异常授权数；配额包括所有墓碑在内的总数 1000、UTC 当日创建数 20，未来时间也进入当日计数并另报异常。恰好耗尽只警告，超过上限才报不一致；剩余额度最低为零，不保证下一次授权成功。

检查批准/派发/发送时序、未来时间、活动请求的计划/任务/结果/证据元数据、连接状态和版本、状态审计是否存在、发送标记和审计时间是否一致，以及未知审计事件。正常终态允许来源已删除；`unknown` 提示 `outcome_unknown_no_retry`，不推断未发生供应商消耗。

过期草稿或授权、超过派发期限的 running、连接过期分别报告 `authorization_expired_pending_cleanup`、`dispatch_deadline_elapsed`、`connection_expired_pending_cleanup`。它们是待业务读取清理的警告，不是审计触发修改的理由。应先通过原请求查询核对；结果不确定时禁止重新使用旧请求发送。新尝试仍需独立预览和精确授权。

`consistent` 不证明建议正确、供应商成功、凭据可用或私有正文已删除。命令不读取模型名、目录、技能名称、自评分数、任务/证据/建议正文、摘要或派发 token，因此也不校验这些私有内容、不检查计划 JSON 内的技能版本和来源自评有效性。完整内容校验仍由业务读写入口负责，实际质量由用户验收。

## 最小列权限

部署者为已有运维角色配置连接权限和凭据，再授予以下列权限。`--user` 是管理员查询范围，不是数据库级租户授权；拥有这些权限的管理员能选择任意用户。

```sql
GRANT USAGE ON SCHEMA public TO learning_reader;
GRANT SELECT (id) ON users TO learning_reader;
GRANT SELECT (user_id,request_id,plan_id,task_id,connection_id,connection_revision,
  status,created_ms,expires_ms,approved_ms,dispatch_deadline_ms,sent_ms)
  ON learning_model_authorizations TO learning_reader;
GRANT SELECT (user_id,request_id,event,at_ms)
  ON learning_model_authorization_audit TO learning_reader;
GRANT SELECT (user_id,request_id,status) ON learning_plans TO learning_reader;
GRANT SELECT (user_id,request_id,id,status) ON learning_tasks TO learning_reader;
GRANT SELECT (user_id,task_id,outcome) ON learning_results TO learning_reader;
GRANT SELECT (user_id,task_id,deleted) ON learning_evidence TO learning_reader;
GRANT SELECT (user_id,id,status,revision,expires_ms)
  ON subscription_connections TO learning_reader;
```

可叠加[学习元数据审计权限](sprint-4-learning-operations.md)。不需要正文、摘要、账户身份哈希、迁移表或任何写权限。

## 可重复开发验收

```sh
make browser-install    # 首次准备锁定的 Playwright / Chromium
make learning-acceptance
```

固定选择 `learning.spec.mjs`，其余沿用隔离 smoke：创建一次性数据服务，执行 PostgreSQL 仓储、学习 HTTP 和只读运维权限测试，构建生产镜像并检查两个审计命令，执行核心 HTTP 流程，再运行学习双入口浏览器验收，最后清理自己的资源。没有真实订阅模型调用；页面模型响应和执行器均使用夹具。其他页面全量、公网、对象存储/向量专项仍通过各自验收入口运行。

新增 HTTP 闭环用例经过实际授权、一次执行、建议回看、人工逐项核验、显式自评确认和删除原证据：模型建议不会改分；确认后分数才改变；删除使关联自评和派生建议失效，独立训练备注保留；旧请求不能重发。另验证超时审计不修改 running，业务读取才将其转为 unknown。运维用例验证最小列权限、私有列和写操作被拒绝、用户范围、分页、大整数、全局问题、墓碑配额和发送审计异常。

## 真实订阅用户验收记录

这部分是待用户执行的验收流程，不属于开发夹具通过的结论。用户先选择已登记连接与可用模型，审阅分享预览，明确同意分享及订阅用量；只批准本次需要核验的证据。不要为了测试取消而额外发起模型请求。

1. 记录代码提交、连接版本、所选模型和请求 UUID；验收记录不附凭据或原始证据。先运行 `audit-models` 保存元数据基线。
2. 在授权到期前，按[本机执行说明](learning-model-execution.md#本机显式执行)运行一次 `learning-run ... --use-subscription`，再用 `learning-show` 或页面核对原请求。命令结果含私有建议，不应粘贴到公共问题或 CI 日志。
3. 若 succeeded，逐项核对建议引用是否确实来自原文、理由是否有依据，并确认自评没有自动变化。若 unknown、invalidated 或 expired，记录原因及元数据，停止该请求；不能把失败当作未消耗用量，也不能重发旧请求。
4. 用户独立填写人工核验与分数，确认后检查学习快照。使用可删除的验收证据时，删除原证据，再检查直接关联自评、建议和引用已清除，训练记录仍在。需要保留证据时，将删除验收明确记为未执行。
5. 再运行 `audit-models`，记录供应商实际用量提示、成功/失败、引用质量、人工确认及生命周期结果。记录未观察到的用量为“未知”，不推断免费或无限量。

真实连接与质量验收未通过前，路线图继续保留对应待办；流式交互与独立本地适配的夹具开发可独立推进。

## 本批验证结果

Rust 1.99 全仓格式、Clippy 和测试通过，前端 lint/typecheck/build 通过。固定学习验收入口通过 162 项 PostgreSQL 仓储、23 项学习 HTTP、10 项学习运维数据库测试、Rust 1.96 生产镜像构建及镜像内两种审计命令、核心 HTTP smoke 和 36 项学习双入口浏览器用例；隔离资源已清理。固定入口也检查了外部 `BROWSER_SPEC` 不会改选其他文件。

本批没有页面实现变更、数据库迁移或真实模型调用；本地未重跑其他页面全量、对象存储/向量专项和公网验收，完整 CI 由推送触发。真实 Pro 可用性、实际用量和建议质量仍待按上述流程验收。
