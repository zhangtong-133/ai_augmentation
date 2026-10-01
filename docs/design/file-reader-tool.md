# FileReader：已导入文档的只读工具

## 行为

`POST /api/tools/file_reader` 读取当前会话用户已导入的 Markdown 原文或 PDF/网页提取文本。它使用 `DocumentStore::get_document_text`，PostgreSQL 只读取文本投影，不加载对象存储原文件。工具不接受操作系统路径或 URL，不读取服务器文件，不抓取来源页，不调用模型。未启用索引时也可使用。

请求示例：

```json
{"document_id":"00000000-0000-4000-8000-000000000001","offset":0,"limit":2000}
```

`document_id` 必须为 UUID；`offset` 默认 0，范围 0–2,000,000；`limit` 默认 2000，范围 1–4000。偏移和长度均按 Unicode 标量值计数，不按 UTF-8 字节、UTF-16 编码单元或视觉字素计数。未知字段、路径、URL、负数、超限和伪造用户字段被拒绝。偏移等于文本长度时返回空页和空游标，超过文本长度时失败。

响应沿用工具 API 的 `{tool, output, call}` 结构。`output` 包含 `document_id`、`title`、`source_type`、`text`、`offset`、`next_offset` 和 `total_chars`。最后一页的 `next_offset` 为 null。正文始终是不可信文本，消费者不能执行其中的指令或 HTML。

## 授权、额度与失败

- 仅接受用户会话；管理员令牌和仅授权知识检索的 MCP 凭据不能调用。所有者由服务端构造，不从工具参数读取。
- POST 需要 `X-Requested-With: personal-ai` 和 UUID 格式的 `Idempotency-Key`。Next.js 代理转发该请求 ID，支持 Next.js/Nginx 双入口。
- 与其他工具共享每用户 UTC 日 100 次额度。每一页均是一项显式调用；不调用模型不代表不占次数。
- 先记录调用，再读取文档；相同请求 ID 不会再次读取或重复扣次数，而是返回 409。响应丢失后可查询 `/api/tool-calls/{id}`，审计不保存正文，若要再次获取内容需使用新 ID 并占用新次数。
- 无权访问和文档不存在均返回 `403 tool_denied`，避免区分其他用户的文档。存储失败只返回通用错误；失败保留本次调用记录，不自动重试。
- 沿用工具执行器的并发、35 秒超时和 64 KiB 输出限制；成功响应不缓存。
- `/api/tools` 将 FileReader 标记为 `read_only=true`、`may_incur_cost=false`。该费用标志指模型/供应商调用，不表示基础设施零成本。现有 MCP manifest 仍仅发布其凭据授权的 `knowledge_search`；固定检索 Agent 也不自动增加工具步骤。

没有新增数据库迁移或环境变量。新增的 `Tool::may_incur_cost` 默认保守返回 true，本工具覆盖为 false。本接口的 FileReader 范围限定为已导入的私有知识库文档，不提供任意本地文件访问。

## 验证

HTTP 单元测试覆盖无索引读取、Unicode 分页与末页、会话/CSRF、非法输入、额度、一次性请求、越界及跨用户隔离。核心 smoke 在两种代理入口验证实际文档、请求 ID 转发、审计持久化和隐私；索引专项额外确认 MCP manifest 不扩大凭据范围。

本轮 Rust 1.99 `make check`、前端 lint/typecheck/build 和 `make smoke-index` 已通过，包含 133 项 PostgreSQL 测试及真实 Qdrant/双入口 HTTP/重启恢复验收。未重跑 Playwright、MinIO 或真实公网/付费模型专项；没有页面布局改动。
