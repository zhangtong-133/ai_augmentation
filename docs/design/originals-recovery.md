# S3 / MinIO 原文备份与恢复

使用 `scripts/originals-recovery.mjs` 同时归档 PostgreSQL 和该快照引用的外部原文。数据库导出及 `originals.json` 使用相同 repeatable-read 快照；下载原文期间持有项目原文维护共享锁，阻止使用同一协议的垃圾回收。正常导入使用唯一对象键，后来的新对象不会混入快照。所有写入/回收进程须使用本仓库协议，桶须由当前数据库独占；外部直接改写或删除对象应先停止。数据库与 S3 没有跨系统事务，不能保证未遵守协议的写入者一致性。

每份备份最多 1000 个引用。只接受 `users/<uuid>/documents/<uuid>/<uuid>` 标准键及 markdown/pdf/web_page 格式；每次只读取一个有界对象，分别限制 256 KiB / 5 MiB / 1 MiB，S3 适配器还限制实际读取流不超过 5 MiB。缺失、超限、校验失败立即停止，失败的新备份目录清理，不留下看似成功的清单。

目录 0700，文件 0600。`originals.json` 与数据库清单绑定；`objects/index.json` 绑定归档和引用摘要，逐个记录大小/SHA-256，正文以对象键 SHA-256 命名的固定 `.bin` 文件保存，拒绝路径跳转、文件及目录符号链接、跨备份清单与损坏数据。对象键也是私有元数据；不输出原文/键/凭据，不上传备份到 CI。摘要是完整性检查，不提供来源认证或加密，归档必须可信并存放于受控介质。

## 命令

先构建 `cargo build --locked -p api-server --bin original-archive`。显式导出 `OBJECT_STORE_ENABLED=true` 及 `OBJECT_STORE_ENDPOINT`、`OBJECT_STORE_BUCKET`、`OBJECT_STORE_REGION`、`OBJECT_STORE_ACCESS_KEY`、`OBJECT_STORE_SECRET_KEY`；使用已配置私有桶，脚本不创建桶、不读取 `.env` 或隐式云凭据。备份配置源桶，恢复前改为专用目标桶，凭据不要写进 shell 命令参数或 Git。

```bash
node scripts/originals-recovery.mjs backup --container "$recovery_container_id" \
  --database personal_ai --directory .backups/personal_ai_with_originals_20261004
node scripts/originals-recovery.mjs verify --directory .backups/personal_ai_with_originals_20261004

# 重新导出 OBJECT_STORE_*，指向专用目标桶后执行
node scripts/originals-recovery.mjs restore --directory .backups/personal_ai_with_originals_20261004
node scripts/originals-recovery.mjs check --directory .backups/personal_ai_with_originals_20261004
node scripts/recovery.mjs restore-with-originals --container "$recovery_container_id" \
  --database personal_ai_restored --directory .backups/personal_ai_with_originals_20261004
```

restore 先验证全部本地文件，再按原键以 S3 原子条件创建写入，逐字节回读验证。已有对象必须与归档一致，内容不同则拒绝覆盖；中途失败保留已写入对象，便于核对或用同一备份继续恢复，不自动删除任何桶对象。check 只读目标对象，缺失或不匹配失败。`restore-with-originals` 先 check，再进行空数据库事务恢复和旧执行授权隔离；操作期间不要开放目标桶给其他写入者。只有备份与目标引用都一致时，才切换应用数据库和桶配置。

单独的 PostgreSQL backup 仍只保存引用；要保存正文须运行上述组合 backup。没有引用文件的历史 PostgreSQL 归档仍可校验恢复，但不能自动生成/验证原文归档。无外部引用时组合备份包含空对象清单。桶属性、IAM/ACL、对象版本历史和外部凭据不在归档中，原文 Content-Type 不用于应用解码（数据库 source_type 决定格式），恢复重新建立权限。Redis 用空实例，Qdrant 使用新集合显式重建。

## 验收

`make recovery-test` 检查跨备份绑定、损坏、缺失、逃逸文件名和符号链接。`make recovery-acceptance-objects` 构建仓库固定官方 MinIO/mc，创建一次性服务，通过应用导入 Unicode 原文，再组合备份到独立目标桶及数据库，验证源垃圾回收锁、0700/0600 权限、目标缺失/冲突拒绝、相同数据重复恢复及重新登录后的私有文档读取。仅使用合成数据和随机凭据，结束只删除本次容器/网络/临时文件。构建需要可用 Docker BuildKit；不会修改系统或常规部署配置。

源站不可达但本机已缓存指定官方 release 镜像时，可先构建上述三个 Rust 二进制，再显式运行 `node scripts/recovery-acceptance.mjs --objects --cached-images`；该选项不拉取其他版本，启动前核对 minio/mc 的精确 release 和 commit。默认 CI 保留源码构建路径。2026-10-04 本机以此方式完成真实原文恢复演练；源码构建因 `proxy.golang.org` 连接超时未完成，不把缓存镜像演练称为本机源码构建验证。
