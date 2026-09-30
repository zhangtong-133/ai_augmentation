# Sprint 3：固定模型计数与单次发送适配

## 当前交付

`personal-ai-llm-openai::replies` 新增 `OpenAiReplyPolicy` 与 `OpenAiReplies`。前者实现内部 `ReplyBudgetPlanner`，后者提供单次 HTTP 发送方法；后续[付费部署入口](sprint-3-paid-replies.md)已组装它们，`CONVERSATION_REPLY_MODE` 支持 `disabled`/`fixture`/`openai`，默认关闭。

首版仅允许官方端点 `https://api.openai.com/v1/chat/completions` 与固定快照 `gpt-4o-mini-2024-07-18`。不接受模型别名、任意兼容地址或客户端模型配置。使用这个快照是为了建立有明确边界的首个纯文本适配器，不代表建议所有任务使用该模型。

## 离线计数与费用上界

锁定 `tiktoken-rs = 0.12.1` 的内嵌 `o200k_base` 词表，启动时加载，不下载词表、不调用计数 API。`count` 核验冻结配置、1–16 条用户消息、连续后缀序号范围、非空/NUL、单条用户文本 4096 字节、系统提示与消息合计 16 KiB、输出上限 1024；文本中的特殊 token 拼写使用普通文本编码。

返回的 `text_tokens` 只统计系统/用户消息正文，不包括服务端角色/边界封装，不能直接用于货币预留。官方[计数说明](https://developers.openai.com/api/docs/guides/token-counting)明确本地文本 tokenizer 不覆盖全部请求结构。因此首版使用固定模型的完整 128,000 token 上下文窗口作为 `input_token_bound`，不以经验消息开销公式或字节数冒充精确计费计数。窗口及快照来自官方[模型说明](https://developers.openai.com/api/docs/models/gpt-4o-mini)。这是基于该模型单次纯文本调用上下文限制的保守上界策略；代价是小请求也会占用较大预留，可信结算后才释放差额。后续如要收紧，须独立证明完整请求上界并升级计数器版本。

计数器版本固定为 `o200k-0.12.1-context-bound-v1`。预算币种为 USD，供应商为 `openai`；部署者必须显式传入价格版本、输入/输出价格、单次和每日上限。代码不内置市场价格，配置版本/价格版本不含凭据。初始化拒绝无法覆盖保守预留的单次限额。每日限额在现有仓储事务中检查。缓存用量按未折扣输入价计算，不因供应商折扣扩大可花额度。

## 单次发送与响应校验

`send_once` 在发送前重新运行策略，冻结预算的全部字段必须与当前策略一致。策略已停用、版本/价格变更、上下文或预算非法时，不发送请求。发送时仅使用冻结的系统和用户文本，固定 `n=1`、`store=false`、`stream=false`、`service_tier=default` 和 `max_completion_tokens=1024`，不提供工具、图像、音频、预测输出或自定义参数。官方[Chat Completions 契约](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)将该输出参数定义为包括不可见生成 token 的上限。

HTTP 客户端禁用环境代理、重定向和 reqwest 自动重试，连接超时 5 秒，总请求/响应读取超时 30 秒，响应体最多 128 KiB；成功正文仍限非空、无 NUL、16 KiB。生产构造器只使用官方 HTTPS 地址；测试通过模块私有构造器连接回环夹具，不调用付费模型。不得把代理重试策略重新加到这条路径。

只接受固定模型、默认服务等级、单个 index=0 的 assistant 文本 choice、`finish_reason=stop`，拒绝拒答、工具/函数/音频结果、截断和畸形 JSON。非 2xx、断线、请求或读取超时归为 `Unknown`，不能推断未计费。错误只返回固定枚举，不包含密钥、上游错误正文或用户文本。

用量必须包含非零整数 prompt/completion token 和一致的 total；缺失、负数、不完整、未知 usage 字段或缓存细分不一致时返回 `usage=None`，仓储全额保留。仅完整合法的用量返回 `ReplyUsage` 供结算。观察到模型/服务等级变化、用量越界或不支持的非零音频/推理/预测计数时返回 `ContractViolation` 并停用共享策略实例；后续计数和发送均拒绝。调用者还可显式 `disable()` 使失效价格配置停止接受新调用。

## 后续执行器接入门槛

本模块不持有数据库发送权，也不保证跨调用幂等。`send_once` 意味着每次调用最多一次 HTTP 尝试；调用者反复调用仍可能重复收费。

已新增[有预算凭据的领取端口及持久化配置登记/停用](sprint-3-reply-dispatch.md)，并已接入[内部执行器](sprint-3-reply-executor.md)，已提供显式金额确认的付费入口；该端口原子返回该请求的冻结上下文及预算，确认事务提交后才能调用适配器。禁止仅凭 `Reply.context`、内存里临时生成的预算或现有无金额历史请求付费发送。领取前要核验配置有效性，取消/删除后不派发或不接收晚到输出，并沿用 120 秒过期收敛与一次性结算。

当前停用标记仅在共享策略实例的进程内有效，重启或其他实例不会继承。开放真实供应商前必须持久化配置停用/价格有效性及告警，处理多实例已领取请求；不能把本地标记描述成全局熔断。随后再增加显式部署配置和页面金额/未知计费提示，完成双入口与故障验收。自动测试始终只使用本地夹具。

## 验收（2026-09-27）

新增 7 项测试：固定模型正文计数/特殊 token 与 Unicode、配置及上下文边界、纯文本输出与 usage 校验、冻结请求字段、HTTP 错误/重定向/超时/超大响应不重发、预算不匹配零请求与契约异常停用、断线未知且不重发。LLM 适配器共 11 项测试通过。

`make check`、前端 lint/typecheck/build、完整 `make smoke` 通过。Smoke 包含 21 项 PostgreSQL、1 项执行器、4 项 Redis 测试，以及双入口 HTTP、重启持久化和缓存故障恢复；新增 tokenizer 依赖的生产镜像构建通过，测试容器、网络与数据已清理。未运行浏览器、MinIO/Qdrant 专项或公网抓取，未调用真实模型。无迁移或运行时配置变化；远端 CI 尚未确认。
