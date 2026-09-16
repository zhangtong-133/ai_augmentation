# Sprint 2：Markdown 导入与文档管理

后续更新：原文对象存储现已实现，启用方式、兼容规则与跨存储事务边界见 [MinIO / S3 原文存储](sprint-2-object-storage.md)。以下保留本阶段交付时的设计。

本轮完成知识导入的第一条流程：登录后上传 UTF-8 Markdown → 解析文本 → 切分 → 持久化 → 查看列表/原文/文本块。

## 接口

所有文档接口使用用户会话 Cookie，管理 Bearer Token 不代替用户身份。

| 接口 | 行为 |
|---|---|
| POST /api/documents | JSON 导入，要求 X-Requested-With: personal-ai；201 返回摘要与 Location |
| GET /api/documents?offset=0 | 当前用户文档，每页 20 条；offset 0–100000，按创建时间及 UUID 倒序 |
| GET /api/documents/{uuid} | 返回当前用户的原文、元数据和有序文本块 |

JSON 字段：title（必填，去除首尾空白后 1–200 字符）、markdown（必填，最大 256 KiB）、source（可选，最多 1024 字节）、tags（可选，最多 20 个，每个 1–40 字符）。标签去除首尾空白、排序去重。正文拒绝空白和 NUL，元数据拒绝控制字符。整个 JSON 请求限 2 MiB，以容纳转义开销。

source 仅作为来源说明保存，不访问 URL、不读取服务器文件路径。浏览器接受 .md/.markdown 文件并严格解码 UTF-8；API 接受 Markdown 字符串，不支持 PDF 或二进制文件。

401：会话无效；403：缺少防跨站请求头；400：非法文档/UUID/分页；409：同一用户已导入相同原文；413：超出大小限制；404：文档不存在或属于其他用户；503：存储不可用。JSON 格式与媒体类型错误沿用原 API 契约。

## 解析、分块与去重

pulldown-cmark 解析 Markdown，提取正文、标题和代码文本；忽略 HTML 事件，不进行 HTML 渲染或脚本执行。块级结束恢复段落边界，CRLF 在解析前转为 LF。原文保持原样。

文本按段落切分，每块最多 1000 个 Unicode 字符；长段落按字符硬切分，避免截断 UTF-8。该长度是字符数，不是模型 token 数；本轮无 overlap、embedding 或向量索引。

去重键是用户 ID + 原文字节 SHA-256。修改标题/标签后再次上传同一原文仍返回 409；不同用户可导入相同内容。不同换行形式的原文视为不同内容。

## 存储边界与阶段性差异

新增 DocumentStore 端口，插入/列表/详情均显式要求 owner。PostgreSQL 适配器每个查询都包含用户 ID；列表不读取原文与 chunks。

0004_documents.sql 新建 documents 表。原文、标题、来源、标签、摘要、创建时间与 chunks 数组通过单条 INSERT 原子保存。唯一约束负责并发去重。

原始 v1 设计要求 MinIO 保存原始文件。本轮为了先打通限长文本导入，将原文暂存在 PostgreSQL，并明确保留为待办。后续应在 DocumentStore 实现中引入 ObjectStorage：先上传对象、写入元数据引用，处理失败补偿与孤立对象清理。领域/API 不绑定供应商 SDK。

## 前端与会话升级

登录后显示 KnowledgePanel，可上传文件、填写标题/标签、分页查看文档并预览原文与文本块。预览使用 React 文本节点，绝不以 HTML 注入。退出会卸载面板，切换用户会重建面板。API 响应 no-store，Service Worker 跳过 /api/。

旧 Cookie 仅允许 /api/auth。本轮改用 personal_ai_session_v2，Path=/api，保留 HttpOnly、SameSite=Strict、Secure 配置。用户需要重新登录；旧 Cookie 不再被接受，旧数据库会话按原期限过期。新名称避免同名不同 Path Cookie 造成歧义。

Next.js 代理新增文档白名单、查询参数转发和流式请求大小检查；Nginx 请求上限同步为 2 MiB。

## 验证与未完成项

HTTP 测试覆盖授权、CSRF、跨用户 404、用户独立去重、超出旧 16 KiB 限制的导入、中文分块、无效输入、超大正文及分页参数。解析器测试验证 Markdown 文本/代码保留及 HTML 块忽略。

PostgreSQL 集成测试入口为 TEST_DATABASE_URL=... make test-postgres，使用可丢弃数据库。正常 cargo test 显式跳过需要数据库的测试；不能将跳过视为持久化验证通过。

后续：真实 PostgreSQL/Compose 联调、MinIO 原文存储、文档删除与重新处理、PDF/网页解析、Embedding 与 RAG。
