# 学习核验私有状态事件

本批在现有一次性执行与授权仓储之上，提供跨进程的状态观察。显式本机执行命令和 API 共用 PostgreSQL，API 定期复核已有状态；不创建执行任务，不领取请求，不调用模型，不保存新的文本副本，无迁移。

## 协议与生命周期

`GET /api/learning/model-authorizations/{request_id}/events` 需要当前用户会话。请求只接受非空 UUID，不接受查询参数或 `Last-Event-ID`；无回放协议。初次检查失败返回普通 JSON 错误（401、404、429 或 503），成功后响应 `text/event-stream`、`Cache-Control: no-store`，禁用代理缓冲。

事件名为 `status` 或 `closed`，SSE `id` 与 JSON `sequence` 相同，均为从 `0` 开始的十进制字符串，仅在当前连接内递增。JSON 示例：

```json
{"protocol_version":"learning-status-v1","request_id":"<UUID>","sequence":"0","detail":{"status":"authorized","terminal":false}}
```

`draft`、`authorized`、`running` 为非终态；其它仓储状态为终态，发出后结束连接。`running` 仅表示已领取，不能据此断定已向模型发送。只发送最新快照及观察到的状态变化，快速连续变化可能合并；不保证逐个交付所有中间状态。业务成功仍须以严格校验并保存后的 `succeeded` 和独立建议查询为准。

每次观察重新验证会话、请求归属和授权来源，复用仓储的过期/失效维护。已建立连接后的失败发送 `closed`，其 `detail.reason` 为 `session_unavailable`、`request_unavailable`、`observation_unavailable` 或 `observation_timeout`，随后结束，不暴露底层错误。来源失效可先表现为 `invalidated` 终态。状态检查后发生的并发变化在下一次观察反映，已经发送的字节不可撤回。

观察最长 20 秒；初始身份验证及每次仓储检查最多 3 秒，后续检查受总截止时间约束。每秒最多一次轮询；状态未变时仅发送 SSE 注释心跳。每 API 进程最多 32 条连接、每用户最多 2 条。连接状态直接由响应体持有，客户端断开、终态或错误都会释放配额，无分离后台任务和待发送队列；未被轮询的响应由 HTTP 连接/代理超时负责释放。限额属于进程，不是集群总预算。

Next 仅对白名单 GET 事件路径使用 30 秒超时并传播请求取消；Nginx 相同路径关闭缓冲/缓存，读取超时 30 秒。普通 JSON 接口保持原有超时。客户端应使用可取消的流式 fetch；结束后查询原请求，不使用自动重连来执行或重发请求。`Last-Event-ID` 会被代理转发并由后端拒绝。

## 数据边界与后续

事件只包含协议版本、当前请求 ID、序号、状态或关闭原因，不包含证据、提示词、增量正文、建议、连接标识、摘要或执行令牌。既有[运行时临时文本通知](learning-model-progress.md)仍限于同进程；本接口不把临时文本写入 PostgreSQL，也不冒充其跨进程传输。

后续先将状态观察接入私有页面，验证切换账户/请求与卸载取消；再单独设计临时正文的跨进程通道、订阅/来源复核及容量边界。最终建议继续从持久化接口读取，之后推进独立本地模型适配、部署恢复和真实订阅质量验收。

## 可重复验收

`make check` 编译并运行非数据库测试。`make learning-acceptance` 在一次性 PostgreSQL/HTTP 栈中运行 `learning_events_*` 测试，覆盖跨连接执行、无自动领取、精简事件、用户隔离、取消、注销、来源删除、超时及连接配额释放；`scripts/smoke-learning-events.mjs` 验证 Next/Nginx 两个实际入口的事件帧、拒绝回放、超过十秒连接和取消终态，并继续执行原学习浏览器回归。全部使用夹具，无真实模型调用。

已接入[私有状态观察页面](learning-status-ui.md)：显式观察/停止，终态后读取原授权，会话失效立即清空学习状态，切换或卸载中断读取。客户端协议限制和双入口 UI 验收见该设计。
