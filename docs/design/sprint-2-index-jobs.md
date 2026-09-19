# Sprint 2：持久化索引任务与自动重试

## 行为

新增整篇文档索引任务。用户提交一次后，API 进程中的后台循环从 PostgreSQL 领取任务，逐批处理已有分块；浏览器不需要保持连接或自行维护 offset。导入仍不会自动触发模型调用。原有 `/index?offset=...` 手动接口继续可用，但不更新持久化任务的进度。

本轮复用 API 中已配置的模型、Qdrant 和文档存储，避免在多个入口重复维护供应商配置。每个启用索引的 API 实例启动一个处理循环，与手动索引共享两个并发许可。`apps/worker` 仍是壳，独立 worker 部署留给后续运行时拆分。任务状态持久化，不依赖进程内队列；多个 API 实例可共同消费同一配置的任务。

## HTTP 接口

| 方法与路径 | 行为 |
| --- | --- |
| `POST /api/documents/{id}/index-jobs` | 幂等创建整篇任务；失败任务从已确认进度恢复；排队/运行任务不重置；完成任务返回原结果 |
| `GET /api/documents/{id}/index-jobs` | 返回当前用户文档在当前索引配置下的任务，无任务时返回 `{"job":null}` |

POST 无请求体，要求登录 Cookie 和 `X-Requested-With: personal-ai`。新任务、运行中任务、失败后恢复返回 202；已有成功任务返回 200。响应包含任务字段，并通过 Location 指向同一路径的 GET。GET 返回 `{"job":{...}}`。两者均 `Cache-Control: no-store`，Next.js 代理已放行；仅入队或查状态，保留普通请求的 10 秒代理超时。

任务字段包括 id、document_id、profile、status、indexed_chunks、total_chunks、attempts、error_code。数据库会验证文档所有权：他人文档与不存在文档统一 404；未登录 401；POST 缺 CSRF 403；UUID 无效 400；索引关闭或任务存储不可用 503。管理员 Bearer 不能替代用户会话。

| status | 含义 |
| --- | --- |
| queued | 待领取，或临时失败后等待退避结束 |
| running | 某处理实例持有有效租约；进程退出后可能保持此状态直到租约恢复 |
| succeeded | 从首块到末块连续确认写入，indexed_chunks 等于 total_chunks |
| failed | 永久数据错误，或本批尝试次数已用尽；再次 POST 可恢复 |

attempts 是当前批次的尝试次数，成功推进一批后归零，不是累计模型调用次数。error_code 只保存固定分类，不存供应商原始报错、内容或凭据。

## 迁移与隔离

追加 `0008_index_jobs.sql`，不改已应用迁移。`document_index_jobs` 保存任务及租约；通过 `(user_id,document_id)` 复合外键绑定文档所有者。删除文档或用户会级联删除任务；已有孤立原文/向量的清理仍属于后续工作。

唯一键为用户 + 文档 + profile。profile 是 Embedding base URL、模型、维度、Qdrant URL 和集合名的 SHA-256，不包含密钥。改变这些设置会生成新的任务配置；历史任务保留，仅由相同配置实例继续消费，GET 只返回当前配置的任务。轮换密钥不会重建任务。等价 URL 的不同拼写（除末尾斜杠外）仍可能形成不同配置，部署时应统一写法。

文档当前不可编辑，任务创建时记录分块总数。后台每批重新以 owner 查询文档，并核对分块数量；不一致直接失败，不伪造完整状态。更换 Embedding 提供方或语义空间时应使用新的模型名/集合，避免不同配置向同一模型的点 ID 写入不兼容向量。

## 领取、重试与恢复

1. 单条 INSERT SELECT 先检查用户所有权，再创建或恢复任务。并发提交同一任务得到同一个 ID；入队不读取 MinIO，不调用模型。
2. 后台用 `FOR UPDATE SKIP LOCKED` 原子领取可运行任务，租约为数据库时间起算的 90 秒，生成新的随机 token，并增加本批尝试次数。
3. 单批处理限时 35 秒，读取文档并调用已有最多 16 块的索引流程。只有 Qdrant 确认写入后，才能结算数据库进度。
4. 成功结算要求 id、token、running 状态及未过期租约同时匹配，并且进度恰好增加当前批次（最多 16 块）。旧租约、重复回执或跳跃进度均被拒绝。末批成功才进入 succeeded。
5. 临时模型故障、限流、向量库故障、文档存储不可用或超时按 5/10/20/40 秒退避；每批最多 5 次尝试。非法分块或 Embedding 形状立即失败。连续进程退出导致租约耗尽，也会终止为 `lease_expired`，不会无限领取。
6. 进程重启或取消后，已确认批次不重做，未确认批次在租约过期后重新领取。显式重试 failed 任务保留已完成 offset，清零当前批次尝试预算；不会重置其他运行者的租约。

队列锁语义参考 [PostgreSQL 16 SELECT 文档](https://www.postgresql.org/docs/16/sql-select.html)，SKIP LOCKED 用于多个消费者避免争抢同一行；普通用户状态查询不使用此选项。

跨 PostgreSQL、模型服务和 Qdrant 仍不是分布式事务。向量写成功但进度未确认时会重做该批，稳定点 ID 防止重复点，但模型调用可能重复计费。租约 token 防止旧处理者推进任务状态，不能撤回已经发送给供应商的请求。成功状态表示任务当时完成写入；不持续监测 Qdrant 数据是否被管理员删除，重复 POST 成功任务也不会强制重建向量。

后台空闲或任务存储失败时每秒轮询。关闭 `KNOWLEDGE_INDEX_ENABLED` 停止处理并禁用任务接口；恢复同样配置后继续消费。无需新增环境变量。现有 readiness 仍仅检查 PostgreSQL；启动时仍沿用上一阶段的 Qdrant 集合验证。

## 验收

- `make test-postgres` 加入 `tests/index_jobs.rs`，使用一次性数据库验证所有权、幂等并发提交、多实例互斥领取、连续进度、过期租约恢复、旧 token 拒绝、配置隔离、退避、最大尝试次数、永久失败与续跑、用户删除级联。
- `make smoke-index` 扩展为真实 PostgreSQL/Qdrant + 本地 Embedding 夹具验收：整篇文档超过一批，两个 HTTP 入口验证认证/CSRF/隔离；夹具先返回一次 429，观察持久化退避状态，再重启 API，验证自动完成全部分块、重复提交不重建任务、向量数量正确。
- 不使用真实模型，也不产生外部 API 费用。未新增页面控件；浏览器显示进度、RAG 检索问答和独立 worker 进程后续实现。

### 本轮结果（2026-09-19）

- `make check` 通过，包含 Rust 格式、Clippy 和工作区测试。
- 前端 lint、typecheck、production build，Compose 配置与脚本语法检查通过。
- `make smoke-index` 通过：新增持久化队列数据库测试、既有 PostgreSQL/Qdrant 测试、双入口任务提交/查询、限流退避、API 重启恢复、整篇多批向量数量与原有 HTTP 持久化流程全部通过；隔离测试资源已清理。
- 未运行 Playwright、真实 OpenAI API、公网网页抓取或 MinIO 专项验收。本轮无页面视觉改动，模型使用本地故障注入夹具。
