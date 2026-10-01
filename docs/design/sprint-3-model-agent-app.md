# Sprint 3：模型 Agent 部署、HTTP 与两阶段费用页面

## 当前交付

对话页面新增“模型知识助手”：生成免费规划报价，明确确认第一阶段金额与调用次数；规划成功后展示固定查询，再生成免费检索回答报价，独立确认第二阶段金额与调用次数。结果展示答案、引用和保存的核验原文。查询、预算与授权规则沿用[内部执行器](sprint-3-model-agent-executor.md)，没有模型自行授权下一阶段的入口。

## 默认关闭与部署

`MODEL_AGENT_MODE` 缺省或 `disabled` 时不创建供应商、不连接额外向量客户端、不登记模型 Agent 配置；真实 PostgreSQL 仓储仍支持历史查询与取消。仅 `openai` 启用付费流程，其他值拒绝启动。启动不发送模型请求。

启用前必须已有匹配的知识索引配置：`KNOWLEDGE_INDEX_ENABLED=true`、`OPENAI_EMBEDDING_MODEL=text-embedding-3-small`、`EMBEDDING_DIMENSIONS=1536`，`OPENAI_BASE_URL` 缺省或官方 `https://api.openai.com/v1`。沿用 `QDRANT_URL/QDRANT_COLLECTION/QDRANT_API_KEY`，检查集合及维度，不自动更换索引模型。普通索引继续使用独立的 `OPENAI_API_KEY`；本功能使用 `MODEL_AGENT_OPENAI_API_KEY`。

`.env.example` 和主 Compose 已提供以下变量；价格和额度没有默认值，必须由部署者核对后显式提供：

| 变量 | 含义 |
| --- | --- |
| `MODEL_AGENT_CONFIGURATION_VERSION` | 配置根版本，最多 110 个可见 ASCII 字符；派生 `-planning/-embedding/-answer` 三个版本 |
| `MODEL_AGENT_CHAT_PRICE_VERSION` | 聊天价格版本 |
| `MODEL_AGENT_CHAT_INPUT_PRICE_MICRO_PER_MILLION` | 每百万输入 token 的 USD 微单位价格 |
| `MODEL_AGENT_CHAT_OUTPUT_PRICE_MICRO_PER_MILLION` | 每百万输出 token 的 USD 微单位价格 |
| `MODEL_AGENT_EMBEDDING_PRICE_VERSION` | 向量化价格版本 |
| `MODEL_AGENT_EMBEDDING_PRICE_MICRO_PER_MILLION` | 每百万输入 token 的 USD 微单位价格；输出费用固定为零 |
| `MODEL_AGENT_PRICE_VALID_UNTIL_UNIX_MS` | 价格有效期，Unix 毫秒；过期拒绝登记/发送 |
| `MODEL_AGENT_PHASE_LIMIT_MICRO` | 每阶段 USD 微单位金额上限 |
| `MODEL_AGENT_DAILY_LIMIT_MICRO` | 同币种共享日账本上限，包含普通回复及其他阶段 |
| `MODEL_AGENT_DAILY_MODEL_CALLS` | UTC 日共享聊天与向量化次数上限 |
| `MODEL_AGENT_DAILY_TOOL_CALLS` | UTC 日工具次数上限，不超过现有 100 次限制 |

数值必须为正整数、在对应存储类型范围内，且通过仓储预算校验。固定模型、计数版本、输入/输出上界由适配器提供，不能由 HTTP 或任意环境模型名替换。启动时先验证适配器，再登记不可变配置；旧版本价格变化、已停用或过期均拒绝启动。修改价格需新版本。注册两阶段是独立事务，后续阶段注册失败会拒绝整个进程启动，不会自动启用部分运行时。

没有 SQL 迁移。密钥不进入浏览器、API 元数据或日志，配置错误只返回变量名或脱敏原因。运维可先以 `MODEL_AGENT_MODE=disabled` 重启停止新批准；配置查询与主动持久化停用命令是下一步。重启不能撤回已发送的供应商请求。

## HTTP 协议

所有路径位于 `/api/conversations/{id}/model-agents`，必须使用登录会话；写入还要求 `X-Requested-With: personal-ai`，沿用现有 CSRF/同源检查。所有者由会话确定，不接受客户端 owner。正常响应均为 `Cache-Control: no-store`。

| 方法与相对路径 | 行为 |
| --- | --- |
| `GET /` | `{ enabled, items: [{ planning, execution }] }`，最多 20 个阶段对；未创建第二阶段时 execution 为 null |
| `POST /` | 只接受 `request_id` UUID 与 `expected_revision`，从部署配置创建规划报价，不调用模型 |
| `GET /{request}` | 读取归属校验后的阶段对，处理既有过期状态 |
| `POST /{request}/approve-planning` | 精确授权规划并后台执行，返回 202 元数据 |
| `POST /{request}/preview-execution` | 空对象 `{}`，从成功规划创建第二阶段报价，不调用模型 |
| `POST /{request}/approve-execution` | 独立精确授权检索回答并后台执行，返回 202 元数据 |
| `POST /{request}/cancel-planning` | 空对象 `{}`，取消未完成规划 |
| `POST /{request}/cancel-execution` | 空对象 `{}`，取消未完成检索回答 |

批准正文只接受 `digest`、`accepted_currency`、`accepted_amount_micro`、`accepted_calls: { chat, embedding, tool }`、`acknowledge_cost`。金额为规范十进制正整数字符串，不接受 JSON Number、前导零、科学计数法或超过 i64 的值。完整内容由仓储逐项与原报价比较；第一阶段授权不能用于第二阶段。创建请求拒绝模型、价格、查询等额外字段。

返回金额同样使用字符串 `amount_micro`，不返回原内部 `amount` 数字，以免浏览器 Number 丢失精度。返回值包含元数据、原查询、已保存证据及合法答案，不包含内部配置、价格、快照、claim 或发送凭据。缺失/外来请求返回 404，格式无效为 400/422，版本/报价/预算冲突为 409，关闭或不可用为 503。

`start_planning/start_execution` 在授权提交后才启动后台任务，不等待供应商完成才返回。授权前还核验请求的冻结配置版本与当前部署版本一致，配置切换时旧报价返回冲突，不先预留费用再拒绝发送。只有第一次批准会启动；后续同键请求返回原记录。授权或领取提交失败不会发送；进程在授权后、领取前退出时不会自动扫描恢复。原同步执行器方法继续用于内部验收。

Next.js 代理只新增上述方法和路径白名单，不转发管理员认证或客户端模型密钥，保留原会话/CSRF 头及默认 10 秒代理超时。真实 API 验收覆盖两次确认，浏览器夹具覆盖交互异常；自动验收不启用真实模型。

## 页面状态与恢复

两个阶段分别展示金额上限及聊天/向量化/工具次数。每次审批弹出独立确认区，复选框不继承上一次确认。按钮重新检查当前消息版本、原报价指纹、金额与到期时间。预览、取消和审批相互锁定，期间不能切换对话、修改消息或并行提交另一助手操作。

使用 BigInt 显示微单位，超过 Number 安全整数范围的报价保持精确。查询和证据仅作为 React 文本呈现，不渲染 HTML；引用白名单不保证答案语义正确。

正在执行时每两秒查询历史，错误立即停止轮询并显示人工刷新入口；终态停止轮询。退出/切换时中止读写等待，丢弃迟到响应。关闭页面或退出不撤销已获授权的后台任务。

写入响应丢失时保留同一个请求 ID、金额、指纹及正文，只有用户显式重试才发送。刷新历史可核对已完成结果，但不会自动触发新的审批。放弃本地未知请求需再次确认，不等同于取消后台任务；用户新建请求可能再次计费。

关闭模式仍能阅读历史、取消未完成请求；空证据、取消、失效、失败、未知结果分别显示状态。供应商或存储异常不会显示为证据不足。

## 验收

新增部署参数测试、3 项真实 PostgreSQL HTTP 测试及配置切换不预留费用回归，以及桌面/窄屏浏览器两阶段确认、超大整数金额、丢失审批响应、取消重试、停止轮询、退出清理、引用文本转义及刷新恢复测试。浏览器成功路径生成 `model-agent.png`。

Rust `make check`、前端 lint/typecheck/build、主 Compose 配置校验通过。完整 `make browser-test` 通过 78 项 PostgreSQL、8 项回复执行器、5 项付费 HTTP（包含首批 2 项模型 HTTP）、2 项运维 CLI、2 项固定 Agent HTTP、4 项 Redis，以及生产镜像、双入口 HTTP、重启持久化和缓存故障恢复。浏览器 40 项通过、8 项按未开启索引/公网开关跳过；新增模型页面在桌面与窄屏各 2 项均通过，截图已检查。

随后补充配置切换前置检查与完整回答 HTTP 回归，最终独立数据库通过 19 项纯逻辑、35 项两阶段 PostgreSQL 专项及全部 3 项模型 HTTP 测试；Clippy 和格式检查通过。测试容器、网络和数据自动清理，构建镜像与本地浏览器报告保留。未调用真实付费模型；未运行 MinIO/Qdrant 专项或公网网页导入。
