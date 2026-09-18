# Sprint 2：Embedding 与 Qdrant 文档索引

后续阶段已补齐[持久化索引任务与自动重试](sprint-2-index-jobs.md)。本文描述保留兼容的同步分批接口，其阶段性限制不代表新增异步任务的能力。

## 本轮范围

实现独立 `EmbeddingProvider` 端口、`personal-ai-llm-openai` HTTP 适配器、`personal-ai-storage-qdrant` REST 适配器，以及已登录用户的显式分批索引接口。原有导入仍只负责可靠保存文档；不会因导入而自动调用付费模型。此轮无聊天、自动问答、搜索 HTTP 接口或新增页面控件。

模型请求使用服务端配置的 OpenAI 兼容 `/embeddings` 接口，发送 `input`、`model`、`dimensions` 和 `encoding_format=float`。选择支持 `dimensions` 参数的模型；模型和维度必须显式配置。返回数据按 `index` 排序，严格检查数量、连续且不重复的索引、模型、维度、有限数值与非零向量，防止错配文本和向量。每批最多 16 段、每段最多 1000 字符，响应最多 4 MiB，请求超时 20 秒；429 单独映射，不自动重试付费调用，不跟随重定向，不透传上游错误或密钥。

接口依据：[OpenAI Embeddings](https://developers.openai.com/api/reference/resources/embeddings/methods/create)、[Qdrant upsert](https://api.qdrant.tech/api-reference/points/upsert-points)、[Qdrant 集合](https://qdrant.tech/documentation/manage-data/collections/)。真实向量库验收使用仓库固定的 Qdrant v1.13.6，使用兼容该版本的 `/points/search` REST 接口。

## 用户与模型隔离

`VectorStore` 从未接线的全局集合参数改为必传 `UserId`。集合、模型和维度在适配器构造时绑定，HTTP 请求不能覆盖它们。记录增加 document_id、ordinal、text、model；payload 写入 owner_id 和 model。写入点 ID 使用用户 UUID、模型名和逻辑块 ID 生成确定性 UUID v5，因此不同用户或模型即使使用相同逻辑 ID 也不会互相覆盖。

检索与删除均由适配器强制构造 owner_id + model 过滤器，调用方不能传入任意过滤器绕过隔离。读取后再次校验 payload 所有者、模型、点 ID 和向量形状。删除同时约束确定性点 ID 与过滤器。REST 操作不跟随重定向，支持可选服务端 API key、10 秒超时及 8 MiB 响应限制。检索上限 20 条，批量删除上限 1000 个逻辑 ID。

启用索引时 API 启动会幂等创建集合，并检查已有集合使用相同维度的未命名 Cosine 向量；不同维度或距离配置启动失败，不重建/删除原集合。同一集合可保存相同维度的多个模型，过滤和点 ID 均区分模型；更改维度需使用新的集合。当前没有 payload 索引调优。

Qdrant 中的 text/source/tags 属于私有知识数据，不应公开暴露数据库端口。之后实现 RAG 时还需根据 PostgreSQL 的文档所有权和存在性复核命中，避免已删除用户/文档的残留向量进入答案。

## HTTP 接口

`POST /api/documents/{uuid}/index?offset=0`，无请求体，必须携带登录 Cookie 和 `X-Requested-With: personal-ai`。管理 Bearer token 不能替代用户会话。Next.js 服务端代理允许该路由，将此请求超时设为 40 秒；其他路由仍为 10 秒。

认证及 CSRF 检查后，先通过 owner-scoped `DocumentStore::get_document` 加载文档；其他用户的文档返回 404，完全不调用模型或向量库。索引已持久化的文本分块，三种来源格式共用相同流程。向量原文不使用原始 PDF 二进制或 HTML 标记。

每次处理从 offset 开始的最多 16 块，使用文档 UUID + 块序号作为逻辑 ID。成功等待 Qdrant `wait=true` 返回 completed 后，返回：

```json
{"document_id":"...","indexed_chunks":16,"next_offset":16,"total_chunks":34}
```

调用方继续请求 `next_offset`，最后一批为 null。`indexed_chunks` 是本次成功数量；`next_offset=null` 仅表示到达文档尾部，不表示之前的批次都已完成。客户端必须从 0 顺序处理，失败后重试同一 offset。重复索引覆盖同一批点，不产生重复点；模型调用仍可能再次计费。

状态码：401 未登录；403 CSRF；400 UUID/offset/查询参数无效；404 文档不属于当前用户或不存在；503 未启用/向量库失败；429 本机并发已满或模型限流；502 模型调用或响应失败；504 本批超时。每个 API 实例最多同时处理两个批次，整个批次（含文档读取）限时 35 秒。响应 `Cache-Control: no-store`。

## 一致性与阶段限制

不新增 SQL 迁移。PostgreSQL 文档不因索引失败而丢失。分批写入不是跨批事务；超时、取消或上游响应丢失时可能已有点写入，使用稳定点 ID 重试即可。此轮不保存 durable 索引任务、完成状态或自动失败重试，不提供完整文档索引完成承诺。并发重试可能重复调用模型，但不会跨用户覆盖或新增重复点。

模型升级和历史文档重建通过显式索引调用完成；删除用户/文档不会自动清理向量，需要后续生命周期清理。异步任务、完整索引状态及 RAG 检索/问答属于下一阶段。

## 配置与运行

默认 `KNOWLEDGE_INDEX_ENABLED=false`。启用时配置服务端环境：

- `KNOWLEDGE_INDEX_ENABLED=true`
- `OPENAI_BASE_URL=https://api.openai.com/v1`（默认）
- `OPENAI_API_KEY`、`OPENAI_EMBEDDING_MODEL`
- `EMBEDDING_DIMENSIONS`，允许 1–4096
- `QDRANT_URL`、`QDRANT_COLLECTION`；可选 `QDRANT_API_KEY`

模型、密钥、维度或集合配置缺失会启动失败。Compose 已传入配置；本机运行使用回环 Qdrant 地址，并按需要通过 NO_PROXY 排除本地服务。API readiness 仍以 PostgreSQL 为准，索引依赖故障由索引请求反馈。

## 验证入口

`make check` 包含 OpenAI 兼容 HTTP 契约、返回顺序与非法形状、错误脱敏，以及 API 的认证、CSRF、跨用户拒绝、offset 校验、重复批次、最后一批及模型/向量依赖失败测试。

新增 `make smoke-index`：独立随机命名的 PostgreSQL/Qdrant/Embedding 夹具/API/Web/Nginx 栈和专用卷。先执行真实 PostgreSQL 与 Qdrant 集成测试，验证集合配置、幂等覆盖、用户和模型隔离、隔离删除；随后验证两个 HTTP 入口、重试后的向量数量，以及 Qdrant/PostgreSQL/API 重启后的持久化。Embedding 使用确定性本地 HTTP 夹具，不代表真实模型效果，也不产生外部 API 费用。全部完成后仅清理本次测试资源。

### 本轮结果（2026-09-16）

- `make check` 全部通过（需要回环端口的测试在允许本机网络的环境执行）。
- 前端 lint、typecheck、production build 和 `make compose-config` 通过。
- `make smoke-index` 通过：真实 PostgreSQL 测试、真实 Qdrant 隔离/幂等测试、Web/Nginx 双入口认证索引、CSRF、重复索引点数，以及向量库/数据库/API 重启持久化均通过。测试容器和卷已按项目隔离清理。
- 未运行真实 OpenAI API、Playwright 浏览器验收或公网网页抓取；本轮模型验证采用本地 HTTP 契约夹具，不验证语义检索质量。
