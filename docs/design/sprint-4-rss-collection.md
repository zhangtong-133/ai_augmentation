# Sprint 4：RSS 订阅与只读采集边界

## 本步交付

新增 personal-ai-feeds 纯 Rust 模块，提供来源 URL 规范化、SubscriptionSnapshot、单次 CollectionPlan、同意摘要和 validate_approval。另已实现[受限 RSS 2.0 解析与去重](sprint-4-rss-parser.md)。模块没有 HTTP、DNS、数据库或模型调用，此纯模块不包含后台任务或页面。另已实现[订阅仓储与一次性采集事务](sprint-4-rss-store.md)，提供内部持久化接口；尚未提供 RSS HTTP 入口或联网采集。

首阶段选择用户手动单次采集。订阅是用户私有数据，默认不自动轮询；订阅 URL 可能含敏感查询参数，不写入公开日志，也不以 Debug 自动输出。RSS 采集不调用模型、不索引知识库、不生成 Daily Brief、不复用站内提醒调度器。

## 已实现：不可变预览和精确同意

调用方从登录态确定 UserId，再从 owner-scoped 仓储加载 SubscriptionSnapshot，不能把请求中的 user_id 或整个快照当成可信来源。快照包括订阅 UUID、所有者 UUID、非零 revision、来源 URL 和 enabled。

plan_collection 为一个非空请求 UUID 生成版本 rss-manual-collection-v1 的预览，绑定规范化来源和订阅版本。有效期为 5 分钟；时间加法检查溢出，接受边界为 created_at <= now < approval_expires_at。SHA-256 摘要来自固定结构 JSON，覆盖全部字段。摘要是精确同意校验值，不是签名、登录凭据或执行许可。

固定策略如下，批准时重新构建并核对，修改策略并重算摘要也不会通过：

| 字段 | 固定值 |
| --- | --- |
| max_response_bytes | 1 MiB |
| max_items | 100 |
| max_redirects | 0 |
| timeout_ms | 8000 |
| fetch_article_pages | false |
| fetch_enclosures | false |
| call_models | false |

validate_approval 要求当前用户、订阅 ID、revision、规范化来源和启用状态与已保存计划一致，拒绝过期、未来计划及不匹配的同意摘要。订阅变更、停用或重新启用均应由未来仓储增加 revision，旧计划不得再次执行。新增 JSON 字段被拒绝。

纯校验不记录使用次数，重复调用校验可以成功；真正的去重、额度、一次性领取和停用并发控制必须由持久化事务实现。客户端只能提交 request_id 和 accepted_digest，服务端重新加载保存的计划。不得接收并执行客户端重建的完整计划。

## 已实现：URL 语法约束；尚未实现：联网许可

只接受 HTTPS 的标准 443 端口、无用户信息的完整 DNS 域名。拒绝 IP 字面量、单标签名称、本地/内部及部分保留后缀、空标签、非法标签、超长输入、控制字符和首尾空白。规范化大小写、默认端口并移除片段，保留路径和查询参数的语义，不删除所谓跟踪参数。

normalize_source 不进行 DNS 解析。通过语法校验的域名仍可能指向回环、私网或保留地址，因此其返回值绝不能直接授权建立连接。

[传输适配器](sprint-4-rss-executor.md)已抽取共用 web-import 的地址策略，并实现以下 RSS 专用约束：

- DNS 最多接受 32 个地址；全部地址须通过公网 IP 检查，混合公网/私网结果整体拒绝。
- 连接固定到本次已验证地址，保留正确的 TLS 主机验证；禁用代理、Cookie、Authorization、Referer 和自动重试。
- 第一版不跟随任何重定向。3xx 作为需要人工更新来源的结果，不修改订阅或发送第二次请求。
- 仅 GET 该来源，8 秒总时限、1 MiB 流式大小上限；请求 identity 编码并拒绝压缩响应。服务器声明的 Content-Length 不能替代实际字节限制。
- 不访问文章、图片、enclosure、cloud、textInput、外部实体或其他文档指定地址。首版不支持带认证头的私有订阅，也不支持条件请求/304；后续增加条件缓存必须绑定用户、订阅、规范 URL 和版本。
- 对来源站只读不代表没有副作用：站点能观察请求。预览应展示来源与限制，由用户明确同意。

## 已实现：受限解析与去重

首版解析范围限定 RSS 2.0。RSS 2.0 的 channel/item 结构及 guid 的不透明字符串语义以 [RSS Advisory Board 规范](https://www.rssboard.org/rss-specification)为依据；不能把 guid 自动当作网络地址。Atom 是另一格式，依据 [RFC 4287](https://www.rfc-editor.org/rfc/rfc4287)，本阶段不宣称兼容。

使用 quick-xml 事件读取及有界中间树解析（具体接收范围见[解析设计](sprint-4-rss-parser.md)）：仅 UTF-8、拒绝 DTD/外部实体、不执行文档内指令；深度最多 32，条目最多 100。标题最多 512 字符，摘要最多 8192 字符。HTML 只转换为受限纯文本，不渲染脚本、样式或远程资源；原始 XML 不进入模型提示词。

条目身份优先使用非空 guid（最多 2048 字节），否则使用合法 HTTPS 链接。首版对既无可用 guid 又无合法链接的条目明确报不支持，不伪造随机 ID。这是产品的受限接收范围，不是对完整 RSS 规范的兼容声明。超限或条目校验失败时拒绝整个批次，不静默提交半个结果。

持久化唯一键应为 (user_id, subscription_id, entry_key)，entry_key 使用带类型前缀的 guid/link 摘要，不能跨用户或跨订阅共享可查询的去重记录。同一来源身份但内容变化记录新内容摘要和更新时间，不新增另一条条目；发布日期仅作来源元数据，不作为授权、排序完整性或去重可信依据。解析结果仍是不可信外部资料。

## 仓储已实现；后续接入 HTTP 状态机

原始实施契约如下；具体表、限额、领取与恢复协议以[仓储设计](sprint-4-rss-store.md)为准。其中同意与领取已合并为一个事务，进程并发上限和 HTTP/CSRF 入口将在执行器及页面阶段接入：

1. 每用户最多 50 个有效订阅，创建不触发采集；名称和 URL 私有，URL 更新/停用增加版本。
2. 预览保存不可变计划；用户确认后事务复核订阅版本及授权窗口，并为 user_id + request_id 建立一次性领取记录。
3. 每用户每日最多 20 次已领取采集，单用户最多 1 个在途任务、进程最多 2 个；网络失败或结果未知仍计次。读取/刷新记录不发出新网络请求。
4. 状态为 draft → approved → running → succeeded/failed/unknown，批准前取消终止；领取后失联不自动重派。再次采集需新的显式预览和同意。
5. 写回时再次检查用户、订阅版本、启用和删除状态。订阅停用/删除先提交时，旧执行结果不得写入订阅条目；已发送的请求不能撤回。
6. 条目 upsert、审计结果和批次终态在同一事务提交。超时或提交结果未知时，先查询原请求；不能自动换 ID 重试。审计不保存 token、完整来源 URL 查询参数、原始 XML 或文章正文。
7. 登录态 API 遵循现有 session/CSRF 边界；预览、确认、状态查询和停用分别独立，工具或模型不能自行批准。

数据库表、迁移及明确的超时未知恢复已落地；HTTP 端点仍待实现。RSS 读取不是现有 MCP knowledge_search 的新权限，也不共享其授权凭据。

## 验证与下一步

本步离线测试覆盖 URL 限制与规范化、账户隔离、订阅变更/停用、精确摘要、策略篡改、未知 JSON 字段、授权过期边界、时间溢出和非法身份。无网络或付费模型测试。

受限 RSS 2.0 解析器与条目身份/去重纯函数已使用本地 XML 夹具验证。订阅仓储与一次性采集事务已落地；[传输与一次性执行器](sprint-4-rss-executor.md)已作为内部库实现，下一步接入 HTTP 和页面。价值评分与 Daily Brief 单独设计预算及人工授权。
