# PostgreSQL 备份与恢复

本批按三阶段交付并分别验收、提交、推送：一致性备份与离线校验 → 空数据库恢复与执行隔离 → 隔离恢复闭环及运维说明。范围为应用 PostgreSQL；S3 原文、Qdrant、外部订阅凭据及配置需另行处理，不将数据库归档称为完整系统备份。

## 备份协议

`node scripts/recovery.mjs backup --container 完整容器_ID --database 应用数据库名 --directory 新备份目录` 使用明确的本机 PostgreSQL 16 容器。仅支持本机 Unix Docker socket 和完整 64 位容器 ID，不接受容器别名、远程 Docker、连接 URL 或默认数据库。容器内客户端走 `127.0.0.1:5432`，从容器环境读取 `POSTGRES_USER`/`POSTGRES_PASSWORD`，不输出凭据、不使用宿主 `.env`。需使用提供这些环境变量和标准 PostgreSQL 工具的服务容器。

从只读 repeatable-read 事务导出快照，`pg_dump --format=custom --snapshot=…` 使用同一快照，元数据与归档一致。源数据库迁移版本和 SHA-384 必须与当前仓库完全一致，PostgreSQL 主版本须为 16。归档保留 PostgreSQL 数据、墓碑和审计，不导出全局角色及权限；恢复须重新配置最小权限。

新目录为 0700，`database.dump` 与最后写入的 `manifest.json` 为 0600；目录必须事先不存在，失败删除本次新建目录，不覆盖旧备份。清单仅包含固定协议、UTC 创建时间、PostgreSQL 主版本、迁移校验、外部原文引用数量和归档大小/SHA-256，不包含数据库地址、账户凭据或正文。

`node scripts/recovery.mjs verify --directory 备份目录` 离线检查清单、当前迁移、文件大小和 SHA-256，拒绝文件符号链接。SHA-256 证明文件与清单一致，不证明来源可信；只恢复自己保存的可信备份。校验不连接 Docker，也不等于已验证恢复成功。

备份包含私有正文、密码摘要及凭据摘要，应存放于受控介质，需要离机时在外部加密。`.backups/` 已由 Git 和 Docker 构建上下文忽略。旧备份仍包含其创建时的数据；删除在线内容不会改写旧备份，需同步执行备份保留/销毁策略，不能将旧快照当作当前删除状态。

## 阶段状态

阶段 1 实现一致性归档和清单、离线校验及一次性 PostgreSQL 验收入口。阶段 2 将提供只恢复到空数据库、恢复前后阻止其他连接和旧任务/授权处理；阶段 3 将补齐登录/私有数据/墓碑/模型一次执行生命周期演练与完整使用说明。

2026-10-04 阶段 1 验收：Node 边界用例与真实 PostgreSQL 16 导出通过；核验所有迁移、标准 custom archive 的业务表、0700/0600 权限及拒绝覆盖旧目录。一次性容器与匿名卷已清理。未改 Rust/前端业务代码，本阶段未重跑 Rust、页面及完整 smoke；恢复及 HTTP 闭环在下一阶段验收。

参考：[PostgreSQL 16 pg_dump](https://www.postgresql.org/docs/16/app-pgdump.html)、[pg_restore](https://www.postgresql.org/docs/16/app-pgrestore.html)。
