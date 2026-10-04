# RSS 评分只读运维

本地 RSS 评分交付后补充独立运维入口：`feed-operations audit-values --user 用户UUID [--after 请求UUID]`。它覆盖全部 RSS 评分模式，和原采集 `audit` 分开；不运行迁移、不恢复超时、不清除记录、不发送推理。采用同一 REPEATABLE READ / READ ONLY 快照，每条 SQL 最长 10 秒，按 UUID 每页最多 100 条；汇总和一致性结果始终覆盖该用户全部记录，不随游标缩小。

报告包含请求标识、状态、创建/到期/批准/发送/派发截止时间、固定问题码、UTC 今日预览和总记录余量。只检查生命周期元数据：首条/当前状态审计、批准/领取对应事件、发送审计恰好一次且时间匹配、未来时间/审计时间异常以及 20 个每日预览/1000 条记录上限。过期待清理、运行超时和未知结果仅列为 warnings；未知结果不得自动重试。`consistent` 只表示这些元数据检查通过，不代表分享正文、供应商配置、分数或材料质量有效。

查询完全不引用 snapshot、scores、pricing、digest 或 dispatch_token 列，也不读取订阅、条目或连接私有字段。因此报告不包含关键词、标题/摘要、模型地址、原文链接、凭据或评分内容。派发执行和阅读仍由原严格协议及归属/来源检查保护。未知用户、非法 UUID/游标或数据库不可用退出 1；成功报告且元数据一致退出 0，即使有 warnings；发现不一致仍输出元数据报告并退出 2。没有 `--apply`、`--retry` 或全用户扫描参数。

```sh
cargo run --locked -p api-server --bin feed-operations -- audit-values --user 用户UUID
# 生产 API 容器已打包 feed-operations：按你的隔离部署入口执行同一参数。
```

只读角色仅需以下列权限（角色创建及密码由部署者自行配置，勿写入 Git）：

```sql
GRANT USAGE ON SCHEMA public TO rss_value_reader;
GRANT SELECT(id) ON users TO rss_value_reader;
GRANT SELECT(user_id,id,status,created_ms,expires_ms,approved_ms,dispatch_deadline_ms,sent_ms)
  ON feed_value_reviews TO rss_value_reader;
GRANT SELECT(user_id,request_id,sequence,event,at_ms) ON feed_value_audit TO rss_value_reader;
```

验证覆盖实际本地授权的 unknown/发送审计缺失、分页全局汇总、过期请求不清理、异用户/无用户隔离；进程测试使用仅获列权限的角色，证明不能读正文、报价、分数和令牌，也不能修改数据，且 0/1/2 退出码稳定。CI 原 `make test-feed-operations` 和 core smoke 自动包含此入口；隔离恢复验收的 `--storage-tests` 也运行只读 CLI 权限检查。

追加阶段验收通过：全仓 Rust/rustfmt/Clippy、168 项仓储（新增 2 项）、6 项连接/评分 HTTP、2 项只读采集/评分 CLI 列权限和退出码、默认关闭的真实恢复演练、完整 core smoke 与生产镜像命令。补充恢复入口对跨用户扫描夹具顺序运行，并保留用例内部并发断言。隔离资源已清理；前端保持上一阶段 lint/typecheck/build 和 26 项 RSS 双入口 UI 的已验收代码，本阶段未重跑浏览器/真实模型/外部原文或向量专项。
