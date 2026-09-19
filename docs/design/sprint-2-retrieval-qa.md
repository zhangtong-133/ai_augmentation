# Sprint 2：检索与引用问答分步交付

## 步骤与验收

1. 整合并验收持久化索引任务、历史原文迁移和孤立对象清理；保留并行开发历史，不改写远端历史。
2. 实现 `POST /api/knowledge/search`。会话身份与 CSRF 校验后，输入 1–1000 字符、limit 1–20（默认 5），向量召回最多 20 块。固定最低余弦得分 0.3；该阈值仅为召回门槛，不表示事实正确或置信概率。逐条按会话所有者读取 PostgreSQL 文本投影，验证文档、序号、模型、文本及逻辑点 ID，去除重复、已删除、过期和无权访问的记录。标题、来源、正文仅来自 PostgreSQL。已索引的有效分块即可检索，进行中的文档可能只有部分结果；不谎称完整覆盖。
3. 实现 `POST /api/knowledge/answer`。独立显式启用聊天配置，复用上述检索（最多 5 块），没有有效命中时返回证据不足且不调用聊天模型。模型只接收问题与当前所有者的核验片段，引用 ID 由服务端分配，返回结果校验引用归属；模型故障、拒答、无效 JSON、未知引用不得返回成功答案。

每步更新路线图、运行 Rust 和前端检查，并使用隔离 PostgreSQL/Qdrant 与本地模型夹具验收后提交推送。真实付费模型不在自动验收范围。此轮交付后端和双代理入口，交互 UI 单独排期。

## 边界

查询使用 POST，避免问题进入 URL/访问日志；响应 `no-store`，不返回向量、凭据或上游错误正文。每进程共用两个索引/检索并发名额，检索总超时 35 秒。原文对象不可用不应影响已持久化文本的检索，因此 PostgreSQL 提供不访问 S3 的文本投影。依赖错误返回失败，不能伪装成空结果。无数据库迁移。

聊天片段是低信任资料，不能覆盖系统指令；不开启工具、网页访问或代码执行。引用有效只证明引用指向本次检索资料，不能机械证明答案语义被资料支持，客户端需展示原文片段供核对。不保存问答历史，不自动创建索引，不自动重试付费请求。

## 检索验收（2026-09-19，WSL / Docker）

`make check`、前端 lint/typecheck/build、`make smoke-index` 通过。单元/HTTP 测试故意让向量假实现返回其他用户和过期载荷，验证数据库复核不会泄露正文，并验证来源取自 PostgreSQL、参数限制、CSRF、模型限流、无效向量、向量故障及空结果。真实 Qdrant 验收覆盖双 HTTP 入口检索与用户隔离，同时回归持久化任务、自动恢复和重启持久化。

`make smoke-objects` 新增文本投影集成断言，验证三种外部原文均可在未配置对象适配器时按所有者读取分块，原文二进制/HTML 不出现在投影中。未运行 Playwright（本轮没有 UI）及真实付费模型；不对搜索相关性或生成答案事实准确率作离线夹具之外的承诺。

## 问答配置与协议

默认 `KNOWLEDGE_ANSWER_ENABLED=false`。启用需要同时设置 `KNOWLEDGE_INDEX_ENABLED=true`、`KNOWLEDGE_ANSWER_ENABLED=true`、`OPENAI_CHAT_MODEL` 和既有模型/Qdrant 配置。聊天与向量化共用服务端 `OPENAI_BASE_URL` / `OPENAI_API_KEY`；聊天模型必须支持 Chat Completions 的严格 JSON Schema 输出，不设隐式默认模型，不回退到无约束文本。配置缺失时启动失败。实现参考 [OpenAI Structured Outputs](https://developers.openai.com/api/docs/guides/structured-outputs)，使用 `response_format`、`max_completion_tokens`，并检查 `finish_reason` 与 `refusal`。

请求示例（带登录 Cookie 及 `X-Requested-With: personal-ai`）：

```http
POST /api/knowledge/answer
Content-Type: application/json

{"query":"我的资料中如何描述这个问题？"}
```

成功响应为 `{"status":"answered","answer":"…","citations":[{"id":1,"document_id":"…","ordinal":0,"title":"…","source":"…","text":"…","score":0.9}]}`。引用由服务端从本次命中分配 1 起始 ID；模型只返回答案、ID 列表及证据不足标志，不能提供自己的来源对象。只有校验过且实际引用的条目才出现在响应里。重复、零值、越界或缺失引用、空答案、超过 4000 字符的答案均返回 502 `invalid_answer`。

无命中或模型明确判定证据不足时响应为 `{"status":"insufficient_evidence","answer":null,"citations":[]}`。没有命中时聊天调用次数为零，但仍需要执行查询向量化；该状态不是对整个知识库内容的不存在证明。模型报告证据不足时必须同时返回空答案与空引用，矛盾输出视为错误。429 限流、502 模型错误与 503 检索存储故障均不伪装成证据不足。

模型输入最多 1000 字符问题和 5 个各不超过 1000 字符的核验片段。输出上限 2048 token（含模型推理 token），HTTP 响应最多 128 KiB，模型请求超时 20 秒，问答总时限 55 秒，Next.js 代理时限 60 秒。默认 Nginx 读取时限 60 秒覆盖 API 预算。不启用流式响应或模型服务端存储，不跟随重定向，不记录问答文本，不持久化聊天记录。调用者应将答案按纯文本展示，并提供引用片段查看，不把模型文本当 HTML 或可执行操作。

该接口只验证引用归属和格式；模型仍可能错误理解资料或生成未被引用支持的句子。实际模型质量、提示注入抵抗效果、模型兼容性与费用需在具体部署模型上单独评估。本地确定性夹具验证协议与隔离，不替代语义质量评估。

## 问答验收（2026-09-19，WSL / Docker）

`make check`、前端 lint/typecheck/build、`make smoke-index`、`make compose-config` 与 `git diff --check` 通过。HTTP 测试验证认证、CSRF、并发额度、默认关闭、空结果零聊天调用、模型故障/限流、缺失/越界/重复/零值引用、超长答案和证据不足一致性。模型 HTTP 适配器测试验证严格 schema 请求、禁用存储/流式、无工具、拒答、截断/非法内容、重定向与错误正文脱敏。

真实 PostgreSQL/Qdrant 与本地 Embedding/聊天 HTTP 夹具覆盖 Next.js 和 Nginx 双入口检索、答案引用正文核对、用户隔离、无匹配资料分支，并回归索引依赖失败恢复、多批次任务与数据库/API 重启持久化。隔离资源已清理。原文迁移和清理在上一检索步骤通过真实 MinIO 验收。本轮未运行 Playwright 或真实付费模型，未新增数据库迁移。
