# 部署数据库只读诊断

`scripts/deployment-check.mjs` 核对显式选择的本机 PostgreSQL 16 应用数据库，使用 repeatable-read 只读事务及 10 秒语句超时。只输出主版本、迁移是否一致、会话/凭据/执行状态等计数和固定问题代码，不输出账户、文档、模型内容、对象键、地址或凭据。不执行迁移、停用、结算、任务发送、上传、删除、服务启动或配置切换。

```bash
node scripts/deployment-check.mjs --container "$recovery_container_id" \
  --database personal_ai --profile current
node scripts/deployment-check.mjs --container "$recovery_container_id" \
  --database personal_ai_restored --profile recovery \
  --originals .backups/personal_ai_with_originals_20261004
```

必须显式指定 profile，容器须完整 64 位 ID，数据库不能是 postgres/template0/template1。连接和凭据规则与[数据库恢复工具](postgres-recovery.md)一致，不读取宿主 `.env`。无外部原文时可用 `make deployment-check CONTAINER=完整ID DATABASE=数据库名 PROFILE=current`，有原文时使用上述 Node 命令，并显式导出目标桶的 `OBJECT_STORE_*` 配置。

current 核对主版本和全部迁移 SHA-384，报告活跃状态；常规活跃会话及已授权任务不视为故障。recovery 用于恢复完成、首次启动 API 前，在此基础上要求会话、活跃 MCP 凭据、启用的持久化付费配置、未结算费用及全部旧待执行任务均为零。恢复后用户重新登录会产生新会话，此时 recovery 不再通过；不要为通过检查而删除合法会话，应切回 current。

存在外部原文时，没有提供归档则给出 `originals_not_verified`。提供归档后，先校验所有文件，再把当前数据库引用与该归档逐项比较，并只读目标 S3 对象验证字节；缺失、错误配置、引用变化或内容不同返回 `originals_mismatch_or_unavailable`。使用与目标数据库一致的组合备份；持续添加新文档后旧备份不再代表当前引用。桶写入者在切换期间应停止，检查只代表检查时点。

输出 `ready=true` 表示本次所选 profile 的数据库及原文检查通过，不能替代 API/网关健康、Redis 空实例、Qdrant 新集合、角色权限配置或游戏显存压力验收。`unknownModelRequests` 可非零，它们是恢复隔离后的未确认历史状态，不能重派，应通过对应审计/账单另行核对。发现问题输出 `ready=false` 并退出 1；工具/数据库不可用时给出脱敏错误并退出 1，不自动修复。

`make recovery-test` 覆盖参数误用及禁止变更开关；`make recovery-acceptance` 验证活跃源库不满足 recovery、隔离目标通过、故意损坏迁移被发现及检查前后业务状态不变。`make recovery-acceptance-objects` 补充未核对/冲突原文失败及完整目标通过。两个演练使用一次性环境，不检查或改写常规部署。

2026-10-04 已通过默认 PostgreSQL、真实 MinIO 及 `--objects --cached-images --local-model` 组合验收：真实核验只发送一次，建议在恢复后保留，证据撤销后失效；原文在独立桶恢复并被诊断验证；所有旧授权/自动任务隔离，检查不改写业务状态。前端 lint/typecheck/build 通过，全仓 Rust 在原文阶段通过；完整远程 CI 另行核对。真实游戏压力和用户材料质量仍需实测。
