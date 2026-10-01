# Sprint 4：RSS 管理与一次性采集 HTTP 接口

## 本步交付

将 [RSS 仓储](sprint-4-rss-store.md)和[受限公网执行器](sprint-4-rss-executor.md)装配到 API，接入 Next.js 代理。订阅、条目、采集历史及审计均属于当前登录用户；不接受 MCP 或 API Bearer 凭据代替登录会话。所有写接口要求 `X-Requested-With: personal-ai`，输入拒绝未知字段，响应禁止缓存。不新增数据库迁移或页面。

`RSS_COLLECTION_MODE` 缺省为 `disabled`，也可显式设为 `public`；其他值导致启动失败。关闭时仍提供订阅管理、离线预览、取消、查询与显式恢复，但确认返回 `503 feed_execution_disabled`，不领取或请求来源。Compose 和 `.env.example` 默认关闭，冒烟栈固定关闭。只有部署者启用 `public`、用户保存预览并明确确认后，才可调用公网传输；不提供内网/夹具/TLS 绕过配置。

## HTTP 协议

以下路径均以 `/api` 开头。UUID 规范化后交给仓储，拒绝空 UUID。版本和毫秒时间戳在 JSON 中采用十进制字符串，避免浏览器数值精度丢失；计数和策略限制仍是数字。客户端回传保存的 digest，不自行对经过字符串转换的计划重新计算摘要。

| 方法与路径 | 输入及结果 |
| --- | --- |
| `GET /feeds/config` | 返回 `execution_enabled` 与 `mode` |
| `GET /feed-subscriptions?after=UUID` | 当前用户订阅分页，返回 `items`、`next_cursor` |
| `POST /feed-subscriptions` | `{id,name,source_url,enabled}`；201，返回保存的订阅和 snapshot |
| `GET /feed-subscriptions/{id}` | 当前用户订阅 |
| `PUT /feed-subscriptions/{id}` | `{revision,name,source_url,enabled}`；revision 为正整数十进制字符串，仓储核对版本并递增 |
| `DELETE /feed-subscriptions/{id}` | `{revision}`；逻辑删除、清理条目，保留私有采集历史 |
| `GET /feed-subscriptions/{id}/entries?after=KEY` | 条目分页；游标为返回的 `guid:`/`link:` 加 64 位小写十六进制摘要 |
| `POST /feed-subscriptions/{id}/collections` | `{request_id}`；201，保存并返回不可变预览和 digest，不联网 |
| `GET /feed-collections?after=UUID` | 当前用户采集历史分页 |
| `GET /feed-collections/{id}` | 读取原请求状态、计数和固定错误原因，不触发执行或恢复 |
| `GET /feed-collections/{id}/audit` | 当前用户请求的私有事件数组 |
| `POST /feed-collections/{id}/confirm` | `{accepted_digest,acknowledge_source_request:true}`；摘要必须为 64 位小写十六进制，且与保存计划精确匹配 |
| `POST /feed-collections/{id}/cancel` | `{}`；仅取消尚未执行的请求 |
| `POST /feed-collections/{id}/recover` | `{}`；由仓储校验 running 超过执行期限后标记 unknown，不重新派发 |

所有列表保持仓储固定每页 20 条。外部没有 claim、finish 接口，也不接受计划、替换来源、响应正文或执行凭据。确认先校验所有权，再检查部署开关；跨用户请求返回 404。仓储继续校验订阅当前版本、启用状态、五分钟预览期限、每日额度和一次性领取，因此旧预览、并发确认和已完成请求均不能重复传输。

成功确认返回保存结果，状态可能为 succeeded、failed 或 unknown；HTTP 200 不表示来源内容一定采集成功。存储冲突返回 `409 feed_conflict`，进程并发占满返回 `429 feed_busy`，存储暂不可用返回 `503 feeds_unavailable`，无法确认写回结果返回 `503 feed_outcome_unknown`。错误不包含 URL 查询参数、数据库信息或网络响应。

## 断开、代理与恢复

确认复用有限执行器：领取最多 5 秒、传输最多 8 秒、写回最多 5 秒。Next.js 仅放行上述方法和路径，确认等待上限 25 秒；仅转发 Cookie、Content-Type 和调用方已有的 CSRF 请求头，不转发 Bearer 凭据。JSON 请求继续使用代理的 16 KiB 上限。

浏览器断开或代理超时后，内部任务继续有限执行。客户端必须查询原 request_id，不自动重新确认或生成替代请求。GET 不修改 running；超过仓储 60 秒执行期限后，用户可显式调用 recover，将仍在运行的记录标记 unknown。未知结果意味着来源可能已收到 GET，不能承诺取消远端请求或自动补采。

## 验证与后续

新增 API 测试覆盖会话与 CSRF、严格输入、版本字符串、跨用户隔离、默认关闭且不领取、精确确认、一次性传输、私有条目与审计、凭据隐藏、未知结果查询及显式恢复。`TEST_DATABASE_URL=… make test-feeds` 使用临时 PostgreSQL 和假传输，不请求真实 RSS；已纳入 Rust CI 和 Compose 冒烟。普通 `make check` 也运行不依赖数据库的鉴权/配置检查。

`make smoke` 在 Next.js 与 Nginx 两个入口分别验证登录、CSRF、订阅创建/更新/删除、预览、关闭状态确认、历史/审计和不可公开的 claim 路径。后续实现订阅及条目页面、来源请求明确确认和原请求恢复交互；自动轮询、价值评分和 Daily Brief 仍未实现。

本步验证通过：Rust 1.98 全仓 `make check`、新增 3 项 PostgreSQL API 集成测试、前端 lint/typecheck/build，以及完整 `make smoke`（含 103 项 PostgreSQL 仓储回归和双入口 RSS 验收）。容器生产构建使用 Rust 1.96。测试全部使用隔离资源，已清理；未请求真实 RSS 或付费模型，未运行浏览器、对象存储和索引专项验收。上一提交 d807570 的远端 CI 五个作业均已成功，包括 objects，无需因本次故障更换存储后端。
