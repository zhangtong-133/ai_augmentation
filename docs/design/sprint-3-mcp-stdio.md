# Sprint 3：本地 MCP 只读知识检索

实现独立可执行程序 `personal-ai-mcp`，通过 stdio 暴露唯一工具 `knowledge_search`。使用[宿主专属最小权限凭据](sprint-3-mcp-credentials.md)，复用用户隔离、每日 100 次工具额度及 PostgreSQL 一次性调用审计。

## 启动与信任边界

先启动配置好索引的 API，再构建：

```sh
cargo build -p personal-ai-mcp --bin personal-ai-mcp
```

由可信本地 MCP 宿主启动 `target/debug/personal-ai-mcp`，使用环境变量传入：

| 变量 | 含义 |
| --- | --- |
| MCP_API_URL | 默认 http://127.0.0.1:8080；仅接受 HTTP IPv4 回环 origin，可指定端口 |
| MCP_ACCESS_TOKEN | 必填；用户通过授权 API 为此宿主签发的 pai_mcp_ 专用凭据 |
| MCP_ALLOW_EMBEDDING_COST | 默认 0；仅显式设为 1 才开放检索 |

凭据通过[登录态授权 API](sprint-3-mcp-credentials.md)签发，仅授予 knowledge_search 权限，通过宿主的秘密环境配置传入。不要写入仓库、命令参数、聊天内容或普通宿主配置文件。撤销与到期由 API 在每次请求时检查，密码重设会撤销全部凭据；普通网页登出不影响独立凭据。API 需监听回环地址，宿主需与 API 同机。此进程仅用于可信本地宿主。旧 MCP_SESSION_TOKEN 配置不再受支持。

启用后，`tools/list` 查询 API 的可用工具；索引未配置时返回空列表。凭据失效返回经过清理的协议错误。默认关闭时不访问 API，工具列表为空，调用失败。

## 调用和费用

工具参数：

```json
{
  "request_id": "a3bd4b8d-1b9e-4a1a-9db2-96d34a0a458a",
  "query": "需要检索的内容",
  "limit": 5,
  "acknowledge_embedding_cost": true
}
```

query 为 1–1000 字符，limit 为 1–5、默认 5。拒绝额外参数，用户身份、上游 URL、Cookie、工具名称和请求头不能由模型覆盖。返回结构化 hits 和 request_id，并提供同内容的文本结果；知识正文始终视作不可信资料。

知识检索会调用向量模型，可能计费。宿主必须向用户展示具体检索并获得同意后再发送；模型填写的布尔值不能证明真人授权。环境开关表示用户信任该宿主执行此约定。此版本复用次数额度，没有嵌入费用的精确金额预留或上限，不能作为无人值守的金额预算控制。

JSON-RPC id 是进程内协议请求标识，必须唯一；request_id 是用户范围内、持久化的工具调用 UUID，映射到 Idempotency-Key。重启桥接后再次使用已执行 UUID，API 仍拒绝派发，不重放缓存结果。失败、超时或丢失响应时不得自动换 UUID 重试；需先通过现有工具审计接口核对，确需新调用时重新获得用户授权。桥接关闭代理、重定向和 HTTP 自动重试，不透传上游错误正文及额外元数据。

## 协议及限制

固定支持 MCP 2025-11-25 的 initialize、notifications/initialized、ping、tools/list、tools/call。客户端须完成初始化协商。按 [stdio 传输](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)、[生命周期](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)和[工具协议](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)实现换行分隔的 JSON-RPC；stdout 仅输出协议消息，错误输出到 stderr，EOF 退出。

每帧最多 16 KiB，超限退出；上游响应最多 128 KiB，HTTP 总超时 40 秒、连接超时 5 秒。单进程串行执行，每个会话最多 4096 个请求 ID，达到上限需重启。拒绝 batch、分页和未知工具；通知不执行工具。没有资源、提示词、采样、MCP Tasks、远程 HTTP/OAuth 或动态工具注册。执行期间不处理取消通知，退出或超时不能撤回已派发的检索，必须核对审计状态。

## 验收与后续

Rust 测试覆盖默认关闭、初始化、非法参数、身份注入、凭据/地址限制、固定请求头、重复调用映射、输出清理及真实 stdio 帧限制。`make smoke-index` 使用真实 PostgreSQL、Qdrant 和本地向量夹具，启动真实桥接进程验证检索、账户隔离、跨进程 UUID 去重及持久化审计，不访问付费模型。

已提供[宿主专属凭据与授权 API](sprint-3-mcp-credentials.md)，后续提供授权管理页面，再评估远程 MCP 传输。此版本不宣称已完成第三方桌面客户端兼容验收。
