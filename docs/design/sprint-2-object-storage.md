# Sprint 2：MinIO / S3 原文存储

## 范围与接口

新增 `personal-ai-storage-s3`，以 `object_store` SDK 实现已有 `ObjectStorage` 端口的 put/get/delete。使用显式 endpoint、bucket、region 和静态服务端凭据，采用 path-style S3 签名请求；供应商类型不进入领域、知识库或 HTTP 接口。客户端请求超时 15 秒，最多重试两次，重试预算 30 秒；错误转换不包含 SDK 响应、URL 或凭据。

`PostgresStore::with_object_storage` 注入可替换的原文端口。Markdown 原始 UTF-8 字节（包括 CRLF）、PDF 原始二进制、网页抓取并解码后的 UTF-8 HTML 分别使用 `text/markdown`、`application/pdf`、`text/html` 内容类型写入私有桶。仍使用已有 256 KiB / 5 MiB / 1 MiB 上限。网页对象并非压缩前的 HTTP 响应字节。

## 持久化与隔离

追加迁移 `0007_document_objects.sql`，增加可空、唯一的 `original_object_key`；已有迁移保持不变。外部原文行的 `original_pdf` 和 `original_html` 为 NULL。提取文本、分块、元数据仍在 PostgreSQL；Markdown 的 `markdown` 字段暂保留一份文本，以维持当前数据结构，原文读取以对象为准。

写入步骤：

1. 校验 ID 与原文格式、大小，生成 `users/{owner}/documents/{document_id}/{attempt_uuid}` 对象键。
2. 开启数据库事务并 INSERT，先通过用户/摘要唯一约束；重复导入返回 409，不上传对象。
3. 上传对象，失败则事务回滚，文档列表与统计不出现半成品；客户端可重新导入。
4. 上传成功后提交数据库事务。只有提交成功才返回 201。

每次尝试的对象键独立，避免重试覆盖另一次已提交的原文。同一原文可以由不同用户分别导入。详情先使用 `WHERE user_id=$1 AND id=$2` 查询数据库，成功后才读取对象；其他用户访问返回 404，完全不发起对象请求。桶名、凭据与对象键不返回浏览器，不新增公开桶、下载链接或原文下载路由。

跨数据库与对象存储没有分布式事务。上传成功而事务失败、连接中断或请求取消可能留下孤立对象；提交失败也可能是提交成功但回执丢失，因此不能直接删除对象。此轮不实现自动清理。后续清理任务必须在足够长的保留期后，根据数据库引用和进行中的写入再次核对；不可仅按一次查询缺少引用就立即删除。删除用户导致的孤立对象也需同类清理流程。

## 兼容与故障行为

`original_object_key IS NULL` 的旧记录继续读取数据库内联原文，不触发外部请求，也不自动迁移历史数据。启用前后 HTTP 请求和 JSON 响应保持一致，原始 PDF/HTML 仍不序列化给浏览器。

外部原文缺失、存储不可用或运行时未配置对象存储时，外部文档详情返回 503，不伪装成不存在或回退到不完整数据。列表、统计及旧记录继续可用。现有 `/api/readyz` 仍只检查 PostgreSQL；对象服务连通性通过实际导入/读取验收。关闭开关不会迁移已存入对象桶的原文，读取这些文档需要重新启用同一个桶。

## 配置与运行

默认 `OBJECT_STORE_ENABLED=false`，保持现有部署行为。启用时必须配置：

- `OBJECT_STORE_ENABLED=true`
- `OBJECT_STORE_ENDPOINT`，例如 Compose 内的 `http://minio:9000`，本机 Rust 使用 `http://127.0.0.1:9000`
- `OBJECT_STORE_BUCKET=personal-ai`
- `OBJECT_STORE_ACCESS_KEY` 和 `OBJECT_STORE_SECRET_KEY`
- `OBJECT_STORE_REGION`，默认 `us-east-1`

缺少配置或布尔开关拼写错误时启动失败；不会静默退回 PostgreSQL。HTTP 支持供本地 MinIO 使用，远程服务配置 HTTPS。endpoint 不接受内嵌凭据、路径、查询参数或片段。本机 Rust 若继承 HTTP 代理，需通过 `NO_PROXY=127.0.0.1,localhost,::1` 排除本地服务；隔离验收自动设置。

`make infra-up` / `make stack-up` 的 `minio-init` 使用管理凭据幂等创建桶，默认不授予匿名权限；启动 API 等待初始化成功。已有桶的权限策略保持不变。示例应用凭据与本地 MinIO 管理凭据一致；自行配置专用应用凭据时需事先创建并授权该桶。外部 S3 桶需要事先由管理员准备，适配器不自动创建桶。

MinIO 镜像采用官方 Quay 仓库相同固定版本，原因是本轮验收中原 Docker Hub 引用拉取失败。出处：[官方容器部署说明](https://github.com/minio/minio/blob/master/docs/docker/README.md)、[mc 官方镜像构建脚本](https://github.com/minio/mc/blob/master/docker-buildx.sh)。适配器配置参考 [AmazonS3Builder 文档](https://docs.rs/object_store/0.12.5/object_store/aws/struct.AmazonS3Builder.html)。

## 验收

`make check` 覆盖适配器键名/endpoint 校验、配置启用规则及现有工作区测试。`make test-postgres` 验证追加迁移与旧存储模式。

新增 `make smoke-objects`：随机命名的独立 Compose 项目、随机凭据、回环随机端口和专用卷，完成后只清理本次资源。先运行真实 PostgreSQL 集成测试，再创建私有 MinIO 桶并运行 `tests/objects.rs`，覆盖三种格式的字节级读取、旧数据兼容、元数据不再内联 PDF/HTML、重复导入不上传、用户隔离先于对象读取、同一内容跨用户独立保存、缺失对象返回不可用、未配置对象存储时的行为，以及缺失桶和注入上传失败后的回滚与重试。随后运行启用 MinIO 的双入口 HTTP 验收，并重启 MinIO/PostgreSQL/API 检查持久化。

Embedding、Qdrant、RAG、历史原文迁移和孤立对象清理留在后续步骤。

### 本轮结果（2026-09-16）

- `make check` 通过；现有回环网络测试在允许本机端口的环境执行。
- 前端 lint、typecheck、production build 全部通过。
- `make compose-config`、验收脚本语法检查与 `git diff --check` 通过。
- `make smoke-objects` 通过：2 个真实 PostgreSQL 测试、1 个包含全部原文场景的真实 MinIO/PostgreSQL 测试，以及 Web/Nginx 双入口 HTTP、CSRF、去重、隔离、统计、重启持久化和退出会话验收。
- 本轮未运行 Playwright 浏览器验收或真实公网网页抓取；页面交互未变。三种原文格式均在存储集成测试中验证，HTTP 重启场景使用 Markdown。
