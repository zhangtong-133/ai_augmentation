# PostgreSQL 备份与恢复

本批按三阶段交付并分别验收、提交、推送：一致性备份与离线校验 → 空数据库恢复与执行隔离 → 隔离恢复闭环及运维说明。范围为应用 PostgreSQL；S3 原文、Qdrant、外部订阅凭据及配置需另行处理，不将数据库归档称为完整系统备份。

## 备份协议

`node scripts/recovery.mjs backup --container 完整容器_ID --database 应用数据库名 --directory 新备份目录` 使用明确的本机 PostgreSQL 16 容器。仅支持本机 Unix Docker socket 和完整 64 位容器 ID，不接受容器别名、远程 Docker、连接 URL 或默认数据库。容器内客户端走 `127.0.0.1:5432`，从容器环境读取 `POSTGRES_USER`/`POSTGRES_PASSWORD`，不输出凭据、不使用宿主 `.env`。需使用提供这些环境变量和标准 PostgreSQL 工具的服务容器。

从只读 repeatable-read 事务导出快照，`pg_dump --format=custom --snapshot=…` 使用同一快照，元数据与归档一致。源数据库迁移版本和 SHA-384 必须与当前仓库完全一致，PostgreSQL 主版本须为 16。归档保留 PostgreSQL 数据、墓碑和审计，不导出全局角色及权限；恢复须重新配置最小权限。

新目录为 0700，`database.dump`、`originals.json` 与最后写入的 `manifest.json` 为 0600；目录必须事先不存在，失败删除本次新建目录，不覆盖旧备份。清单仅包含固定协议、UTC 创建时间、PostgreSQL 主版本、迁移校验、外部原文引用数量和归档/引用文件的大小及 SHA-256，不包含数据库地址、账户凭据或正文。引用文件含私有用户/文档对象键，应与归档一起保护；首版导出最多支持 1000 个外部原文引用，超出拒绝备份。

`node scripts/recovery.mjs verify --directory 备份目录` 离线检查清单、当前迁移、文件大小和 SHA-256，拒绝文件符号链接。SHA-256 证明文件与清单一致，不证明来源可信；只恢复自己保存的可信备份。校验不连接 Docker，也不等于已验证恢复成功。

备份包含私有正文、密码摘要及凭据摘要，应存放于受控介质，需要离机时在外部加密。`.backups/` 已由 Git 和 Docker 构建上下文忽略。旧备份仍包含其创建时的数据；删除在线内容不会改写旧备份，需同步执行备份保留/销毁策略，不能将旧快照当作当前删除状态。

## 阶段状态

三阶段 PostgreSQL 范围已交付：一致性归档及离线校验、空库恢复与旧任务/授权处理、登录/私有数据/墓碑/旧模型授权不执行的应用演练。后续增加真实本地模型完整执行、外部原文备份和部署诊断。

2026-10-04 阶段 1 验收：Node 边界用例与真实 PostgreSQL 16 导出通过；核验所有迁移、标准 custom archive 的业务表、0700/0600 权限及拒绝覆盖旧目录。一次性容器与匿名卷已清理。未改 Rust/前端业务代码，本阶段未重跑 Rust、页面及完整 smoke；恢复及 HTTP 闭环在下一阶段验收。

## 空库恢复与执行隔离

`node scripts/recovery.mjs restore --container 完整容器_ID --database 空数据库名 --directory 备份目录` 先离线校验，再确认目标为 PostgreSQL 16、没有用户对象或其他客户端连接。不得使用在线应用数据库，命令不支持 `--clean` 或覆盖模式。恢复工具通过目标会话锁和容器内 `postgres` 维护连接临时禁止目标的新连接，原有非空数据库的拒绝不会修改数据或连接策略；需要容器管理员权限。

custom archive 解码、实际迁移/外部原文数量复核及 `infra/postgres/recovery-quarantine.sql` 在同一事务执行。读取归档时再次计算 SHA-256，阻止校验后文件变化被提交。事务完成并确认执行隔离后，才恢复目标连接；导入失败不会提交部分表，已进入恢复的空目标保持 `ALLOW_CONNECTIONS=false`。应保留失败目标用于核对，或删除明确属于本次恢复的空目标重新创建；不能在结果未知时直接启动服务。

隔离策略为 `offline-quarantine-v1`：清除所有会话、撤销 MCP 凭据、停用持久化付费配置；暂停未完成索引，取消待回复/计划/提醒及周期采集、关闭规则日报；待核验授权失效，执行中的模型/工具/RSS 请求转为未知或超时，保留已保存发送标记及次数账本。正常终态与用户独立数据、墓碑和已完成建议不改写。未结算金额按全额 `retained` 保留，不退款、不伪造发送时间；恢复快照不能证明之后供应商实际用量，启用新付费配置前须另行核对账单。

源快照可能包含仍活跃的授权，但恢复后不能继续这些旧意图。需要重新登录、签发凭据、选择新配置或创建并批准新请求；不重新领取旧 `running`/`unknown` 请求。订阅 OAuth 文件不在归档中，连接表只含元数据，不自动恢复凭据。

若清单含外部原文引用，恢复默认拒绝。建议使用[原文备份恢复](originals-recovery.md)逐字节核对目标桶，再执行 `restore-with-originals` 自动复核并恢复空数据库。`--external-originals-ready true` 仍仅是操作人声明，不自动校验 S3；只用于另有外部核对流程的旧归档。Qdrant 向量需使用独立新集合重建，Redis 应使用空实例，避免旧缓存或临时正文与恢复后的数据库混用。

2026-10-04 阶段 2 验收：一次性 PostgreSQL 中恢复全部业务表，验证拒绝非空/占用目标、篡改提前拒绝、有效校验但非法归档不提交且目标保持离线，以及会话/凭据/自动任务/模型状态/发送标记/费用与次数保留。阶段 2 未改 Rust/前端业务代码；应用 HTTP 恢复闭环在阶段 3 验收。

## 常规使用

备份时只运行导出命令，不切换服务、不暂停模型或采集。快照只代表备份时点；如需与外部原文保持一致，须单独阻止原文写入/删除并准备对应对象。以下命令由操作人选择实际服务和数据库，开发验收不使用常规项目。

```bash
mkdir -p .backups
recovery_container_id=$(./scripts/compose.sh ps -q postgres)
node scripts/recovery.mjs backup --container "$recovery_container_id" \
  --database personal_ai --directory .backups/personal_ai_20261004_120000
node scripts/recovery.mjs verify --directory .backups/personal_ai_20261004_120000
```

日期目录仅是示例，改为本次唯一名称。准备独立空目标时，从维护数据库执行 `CREATE DATABASE personal_ai_restored TEMPLATE template0`，无需删除现有数据库。然后：

```bash
node scripts/recovery.mjs restore --container "$recovery_container_id" \
  --database personal_ai_restored --directory .backups/personal_ai_20261004_120000
```

确认命令返回 `restored=true` 与 `quarantined=true` 后，先运行[只读部署诊断](deployment-check.md)的 recovery profile，核对隔离状态及目标原文；通过后在本机配置把应用 `DATABASE_URL` 的数据库名切换到新库。先用关闭模型/索引/RSS/scheduler 的配置启动 API，使用空 Redis、新 Qdrant 集合及已核对的 S3 桶；重新登录并只读核对用户数据、墓碑和审计。旧会话必须失败，旧待核验授权必须 invalidated/unknown，不能复用。新模型请求或周期任务须重新审阅授权，索引须显式重建；重新配置最小数据库角色权限，归档没有恢复 ACL。保留原数据库和备份，切换失败时不要覆盖原库。

如恢复导入失败，目标可能保持禁止连接；使用容器内 `postgres` 维护数据库查询 `pg_database.datallowconn`。先核对目标确属本次恢复，再决定保留、删除重建，或在确认事务结果后用 `ALTER DATABASE personal_ai_restored ALLOW_CONNECTIONS true` 开放。该维护操作不会自动恢复授权或重新发送请求。

## 可重复演练

```bash
make recovery-test
make recovery-acceptance
```

后者构建当前 API 与 local-review 二进制，创建随机命名的 PostgreSQL 容器和专用匿名卷。通过 API 使用随机密码的合成账户创建文档、删除会话、完成训练、核验/确认自评再删除证据，并保存本地模型授权；随后备份、恢复、重新启动 API 验证原会话失效、重新登录和用户隔离、墓碑及原授权不再批准/执行。模型服务无需启动，不产生推理请求。还覆盖所有自动任务暂停与费用保留、忙碌目标/非空目标拒绝、损坏归档不提交。结束仅停止本次 API、删除本次容器/卷和临时文件，不写常规 `.env`；不上传归档、认证状态或私有正文到 CI artifact。

2026-10-04 阶段 3 的实际 PostgreSQL/API 演练通过，已接入 Rust CI 任务。全仓 Rust 和前端检查单独执行；没有新增迁移、UI 行为或真实模型调用。本阶段未重跑完整浏览器、对象存储、向量及公网专项；当前远程完整 CI 状态须单独核对。

追加阶段 4 已通过 `make recovery-acceptance-local`：先显式启动项目模型监督器，再以合成证据通过 HTTP 创建精确本地授权，由受显存保护的执行命令完成真实 llama.cpp/Qwen 推理。重复原请求读取相同保存建议，备份恢复后仍读取已完成结果，删除来源证据后建议清除且原请求不能重发；数据库发送审计始终只有一次，自评快照没有自动改变。该命令独立于默认 CI，无模型服务时不会偷偷启用其他模型；运行结束应执行 `make local-model-stop`。此次验收不替代真实用户材料质量及游戏峰值压力测试。

参考：[PostgreSQL 16 pg_dump](https://www.postgresql.org/docs/16/app-pgdump.html)、[pg_restore](https://www.postgresql.org/docs/16/app-pgrestore.html)。
