# Sprint 3：显式付费回复部署与金额确认

## 行为

`CONVERSATION_REPLY_MODE=openai` 现在可显式组装固定模型策略、预算仓储和供应商执行器。默认仍为 `disabled`；`fixture` 保留本地测试行为。新付费请求先由用户确认金额和配置版本，再在同一数据库事务中检查配置、预留次数和金额。付款金额以整数微美元表示，1 USD = 1,000,000 微美元；页面与 HTTP 使用十进制字符串，避免 JavaScript 大整数精度损失。

页面展示每次预留额、单次配置上限、每用户 UTC 日上限、历史预留及结算金额。每次用户显式确认才创建新 ID；失去响应后的重试冻结原 ID、消息版本、配置版本及金额。报价变化后必须重新确认。配置关闭后，仍可重放原请求、查看历史或取消；无金额历史请求不会被升级为付费调用。

付费上下文只包含受限的已保存用户消息及固定系统提示，不含知识库、长期记忆或已有助手回复。页面说明消息将发送给 OpenAI，超时/未知结果或派发后取消可能已计费，并保留全部预留。金额限制只覆盖本功能，现有知识问答及 Embedding 使用各自配置。

## 部署配置

本机 Rust 进程需要导出变量；Compose 会从 `.env` 传入 API 进程。新增变量均无价格或密钥默认值，只有 `openai` 模式才读取。配置缺失、金额非正整数/溢出、限额不足、价格过期、配置已停用或同版本内容变化时拒绝启动。

| 变量 | 内容 |
| --- | --- |
| `CONVERSATION_REPLY_MODE` | `disabled`、`fixture` 或 `openai` |
| `REPLY_OPENAI_API_KEY` | 本功能独立的服务端 OpenAI 密钥 |
| `REPLY_CONFIGURATION_REVISION` | 管理员生成的非敏感唯一配置版本，最多 128 字节 |
| `REPLY_PRICE_VERSION` | 管理员核验的非敏感价格版本，最多 128 字节 |
| `REPLY_INPUT_PRICE_MICRO_PER_MILLION` | 每百万输入 token 的微美元价格 |
| `REPLY_OUTPUT_PRICE_MICRO_PER_MILLION` | 每百万输出 token 的微美元价格 |
| `REPLY_REQUEST_LIMIT_MICRO` | 单请求微美元限额 |
| `REPLY_DAILY_LIMIT_MICRO` | 每用户 UTC 日微美元限额 |
| `REPLY_PRICE_VALID_UNTIL_UNIX_MS` | 管理员审核的价格有效截止时间，Unix 毫秒 |

供应商端点、模型快照及计数器由代码固定，沿用[供应商设计](sprint-3-reply-provider.md)；本功能不读取 `OPENAI_BASE_URL`、`OPENAI_CHAT_MODEL` 或通用 `OPENAI_API_KEY`。单次预算使用完整上下文窗口输入上界及输出硬上限，部署限额必须覆盖其保守预留。代码不内置当前报价，也不会自动探测供应商调价，管理员需要核对供应商价格并设定有效期。

启动时登记不可变配置及截止时间，执行器每秒扫描一次。新价格、额度或截止时间需要新的配置版本；同版本重启只能重放完全相同配置，不能延长截止时间或重新启用已停用版本。密钥轮换可以沿用相同价格配置。不同版本在同币种 UTC 日账本上继续累计，不重置已用额度。

## 停用与恢复

供应商模型/服务等级/usage 契约异常会停用当前实例及数据库配置，错误日志带配置版本，便于关联账单核对。停用持久化失败时本实例停止接受新请求和发送，后续 tick 重试写入。其他实例只有在数据库停用成功后才能观察到该状态。

管理员可使用[回复运维命令](sprint-3-reply-operations.md)查询配置与账本。停用默认预览，明确添加 `--apply` 才写入：

```bash
cargo run -p api-server --bin reply-operations -- disable "$REPLY_CONFIGURATION_REVISION"
cargo run -p api-server --bin reply-operations -- disable "$REPLY_CONFIGURATION_REVISION" --apply
```

配置停用或过期后，新请求的事务预留和领取均拒绝，页面禁用新请求按钮。已有 queued 请求可显式取消以退回次数/金额；派发后的取消或未知结果保留全额。停用不能撤回已经发出的网络请求。若服务重启时原配置已失效，可设置 `CONVERSATION_REPLY_MODE=disabled` 启动并管理历史；核对价格和账单后，以新的配置版本重新启用。

## HTTP 契约

列表响应新增 `quote`，包含 `configuration_revision`、`currency`、`reservation_micro`、`request_limit_micro`、`daily_limit_micro`。配置已失效时 `enabled=false`，历史仍保留。每条有金额记录的回复新增 `billing`：`currency`、`reserved_micro`、可空的 `charged_micro` 与 `settlement`。这些字段不含内部预算 JSON、计数器、供应商密钥或冻结上下文。

创建新付费请求在原 `request_id` / `expected_revision` 之外必须提供 `configuration_revision` 和 `accepted_max_micro`，并与当前报价完全一致。缺失或变化返回 409。客户端价格、模型等未知字段拒绝。原 ID 先查归属和消息版本，若已有持久化请求则返回原记录，不依赖当前启用模式或报价，不重复预留。

新请求通过 `reserve_active_budgeted_reply` 在用户→对话锁后，锁住有效配置并核验完整预算，再预留次数和金额；配置停用与该事务通过配置行锁排序。完成预留不能替代派发授权，执行器继续使用独立的预算领取及发送前复核。取消、删除和迟到结果沿用既有终态及一次性结算保护。

## 测试边界

新增 PostgreSQL HTTP 测试使用真实固定模型离线计数策略和本地供应商夹具，覆盖金额确认、认证/CSRF、归属隔离、额度回滚、取消退款、失效配置及关闭模式后的历史重放。`make test-replies` 统一运行执行器与付费路由测试，已接入 smoke 和 CI。

新增双入口浏览器用例使用真实账户/对话操作，并在浏览器端提供报价/结算响应夹具，验证确认、报价变化、响应丢失后原金额重试、停用提示、大整数金额展示和退出清理。原 HTTP 代理、服务重启及持久化由完整 smoke 验证。自动验收不使用真实付费供应商。

## 验收（2026-09-30）

`make check`、前端 lint/typecheck/build 和 `make compose-config` 通过。`make browser-test` 通过，包含 25 项 PostgreSQL、8 项执行器、3 项新增付费 HTTP、4 项 Redis 测试、生产镜像构建、双代理 HTTP、重启持久化及缓存故障恢复；浏览器 30 项通过、6 项索引/公网专项按默认开关跳过。新增金额用例在桌面及窄屏都通过，已检查 Playwright 生成的截图。

首次完整验收因旧 HTTP 列表断言缺少新增的 `quote` 字段停止；更新断言后重跑通过。专项测试和完整验收的临时数据库、容器、网络及数据卷均已清理。

没有新数据库迁移。未运行真实索引/MinIO/Qdrant 或公网抓取专项，未调用付费模型；远端 CI 尚未确认。部署默认保持关闭，启用时必须由管理员提供独立密钥、已核验价格、额度及有效期。
