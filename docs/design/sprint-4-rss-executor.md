# Sprint 4：RSS 公网传输与一次性执行器

## 本步交付

新增 personal-ai-feed-http 的 PublicFeedTransport，实现 personal-ai-feeds::transport::FeedTransport；新增 personal-ai-agent-core::feeds::FeedExecutor，将[持久化授权](sprint-4-rss-store.md)、单次传输和结果写回串联。公网 IP 分类抽取到 personal-ai-public-network，与已有网页导入共用，网页导入原有地址策略保持不变。

目前仅交付内部库和测试，没有将执行器接入应用入口、HTTP、后台轮询或页面，不增加配置和数据库迁移。未来服务端在会话与 CSRF 校验之后，才可以调用 execute(owner, request_id, accepted_digest)；不能从客户端接收完整计划、执行凭据、DNS 结果或响应正文。

## 固定传输边界

来源先经过 normalize_source：只允许 HTTPS、443、完整 DNS 域名，不允许 IP 字面量、用户信息、本地域、控制字符或超长 URL。域名语法合格不等于允许联网；每次传输还必须完成 DNS 地址检查。

使用系统 DNS 一次解析，最多接收 32 个结果。空结果失败；任一地址属于私网、回环、链路本地、组播、保留、文档或过渡地址时整组拒绝，不从混合结果挑选公网地址。公网分类采用原网页导入的保守规则；参考 [IANA IPv4 特殊用途注册表](https://www.iana.org/assignments/iana-ipv4-special-registry/)与 [IPv6 注册表](https://www.iana.org/assignments/iana-ipv6-special-registry/)，包括拒绝 IPv4 映射 IPv6、NAT64/6to4 和非普通 IPv6 全球单播地址。部分特殊用途但可路由地址也主动拒绝，不承诺全部公网地址兼容。

整组通过后，仅选择第一个地址，固定到本次新建客户端；连接失败不切换备用地址、不再次解析。URL 中保留原域名，TLS 证书和主机名验证保持开启。生产入口不暴露自定义根证书、DNS 注入、代理、内网白名单或关闭 TLS 校验的开关。

| 边界 | 实现 |
| --- | --- |
| 请求 | 一次 HTTP/1.1 GET，只访问来源 URL |
| 重定向 | 禁止跟随；所有 3xx（含 304）失败 |
| 重试 | reqwest::retry::never()；新客户端、单固定地址，无应用重试 |
| 总超时 | 8 秒，包含 DNS、连接、TLS、响应头和流式正文；连接另限 3 秒 |
| 正文大小 | 1 MiB，检查 Content-Length 并逐块计数；不依赖声明长度 |
| 编码 | 请求 identity；关闭自动解压，拒绝非 identity 响应；仅 UTF-8 字节 |
| MIME | 单个 application/rss+xml、application/xml 或 text/xml；可选单个 charset=UTF-8 |
| 状态 | 只接受 200；拒绝部分响应及 Content-Range |
| 私有信息 | 不转发 Cookie、Authorization、Referer 或代理凭据，不发送条件请求头 |
| 返回 | 原始有界字节交给已有解析/写回事务，不获取文章、图片或 enclosure |

MIME 参数的未知、重复或非法形式以及重复 Content-Type 被拒绝。这是明确受限接收范围；不支持压缩订阅、Atom MIME、重定向源或认证源。错误只有固定枚举，不携带 URL 查询参数、响应正文或 reqwest 原始错误。

## 执行、断开与未知结果

FeedExecutor 使用进程共享的两个许可；新建多个执行器不能扩容。许可在调用 claim_collection 之前获取，名额不足直接 Busy，不领取、不消耗每日采集额度。PublicFeedTransport 也有独立的进程共享传输上限，防止内部调用绕过传输并发限制。持久化层继续保证单用户最多一个 running 请求。

执行步骤：

1. 获得进程许可后启动内部任务，用 5 秒上限领取请求。仓储负责当前用户、订阅版本、有效期、精确同意、每日额度和一次性 claim_id；任何失败均不传输。
2. 只用返回计划的保存来源调用一次 FeedTransport，执行器额外施加 8 秒上限，避免其他适配器无限等待。不能替换来源或放宽固定策略。
3. 成功字节交给 finish_collection；已知策略/响应拒绝记录 failed/transport，超时或无法判定网络结果记录 unknown/unknown。仓储解析失败仍记录 failed/parse，并再次复核订阅版本/停用/删除边界。
4. 写回事务等待最多 5 秒。领取或写回超时、内部任务异常返回 OutcomeUnknown；存储错误原样返回脱敏 Storage 错误。均不重试领取、不重发网络请求，调用方应查询原 request_id。

HTTP 调用方取消等待或断开连接，只丢弃外层等待；已经启动的内部任务继续持有并发许可，按上述有限步骤尝试完成写回。不会因为页面刷新而重新执行。进程退出或内部任务异常可能留下 running，由既有 60 秒期限后的显式 recover_collection 标记 unknown；不自动重派、不退额度。

这保证的是本系统不自动重复发送，不承诺远端站点的副作用可撤销。已发出的 GET 即使超时，远端也可能已经收到。删除/停用之后的旧执行结果不能写回，但此前已发送的请求无法撤回。

## 测试与 CI 排查

传输测试全部使用本地 TLS 夹具或假 DNS，不请求外部 RSS。测试覆盖来源先于 DNS 校验、混合/超量 DNS 拒绝、固定端点与原 Host、TLS 信任/主机名验证、凭据头缺席、重定向与连接失败不重试、压缩/MIME/UTF-8 拒绝、精确 1 MiB 边界、chunked 超限、截断响应以及 DNS 总超时。测试证书和私钥是公开夹具，测试代码单独信任根证书，生产客户端不加载它。

新增执行器 PostgreSQL 测试验证精确同意、已完成/失败请求不重发、跨实例进程并发上限、名额不足不领取、调用方断开后继续有限写回、网络超时未知及写回失败不重试。数据库使用本任务独立 PostgreSQL 16 临时容器。

排查 [1568364 的 CI](https://github.com/zhangtong-133/ai_augmentation/actions/runs/36815577887) 和 [d3eaec3 的 CI](https://github.com/zhangtong-133/ai_augmentation/actions/runs/36816756383)：对象存储 objects 作业均成功。失败来自 stable 已升级到 Rust 1.98，新 Clippy manual_assert_eq 在 RSS 测试中要求 assert_eq!/assert_ne!；本地原 Rust 1.96 没有该检查。本步修正五处断言，并安装官方 Rust 1.98 运行全仓检查验证，没有屏蔽 lint，也没有改变对象存储后端。现有证据不支持为了本次 CI 修复迁移到 RustFS。

Rust 1.98 的 make check、完整 PostgreSQL 回归 103 项、Rust 1.96 全仓 Clippy，以及前端 lint/typecheck/build 均通过。未新增 HTTP/页面，未重复运行 Compose 浏览器或对象存储 smoke；没有访问真实订阅或调用付费模型。

下一步接入 RSS 会话/CSRF 保护的管理/确认/状态接口，再实现订阅页面和明确的来源请求确认。自动轮询、价值评分、Daily Brief 仍未实现。
