# RSS 订阅评分用户 HTTP

在[评分仓储和内部执行器](rss-value-execution.md)上提供登录用户自己的订阅评分预览、精确同意、取消和结果查询。网页授权不触发模型；执行仍使用[本地显式命令](rss-value-local.md)。没有新增迁移、凭据上传、后台扫描或 API 金额执行路径。

## 接口

| 方法与路径 | 输入 | 行为 |
| --- | --- | --- |
| `POST /api/feed-values` | `id`、`connection_id`、`connection_revision`、`model` | 从当前用户数据库候选生成不可变预览 |
| `GET /api/feed-values` | 可选 `after=UUID` | UUID 升序，每页 20 条摘要及 next_cursor |
| `GET /api/feed-values/{id}` | 无 | 私有详情、精确分享内容、候选映射和已校验评分 |
| `GET /api/feed-values/{id}/reading` | 无 | 已完成评分的私有阅读投影，含摘要、原文链接与两种分数 |
| `GET /api/feed-values/{id}/audit` | 无 | 私有状态/时间审计 |
| `POST /api/feed-values/{id}/approve` | `digest`、`acknowledge_sharing`、`acknowledge_subscription_usage` | 精确摘要与两项明确同意，不执行模型 |
| `POST /api/feed-values/{id}/cancel` | 空对象 `{}` | 幂等取消尚未完成的计划并清除内容 |

所有成功响应为 200。UUID 使用非零值；连接版本必须是规范十进制字符串（例如 `"1"`）。模型为非空、无控制字符且最多 128 字节的字符串。首次预览在可信服务器中从当前连接构造订阅模式报价，必须匹配请求选择的连接版本和模型；预览与批准事务继续验证归属、期限、版本和完整快照。客户端不能提供 owner、供应商、价格、金额、正文或凭据。

同一个预览 ID 返回原记录，不能借重放更改模型、摘要或有效期；原连接发生变化时应查询原 ID，再显式创建新预览。批准失败不能自动接受新 digest。取消/撤销之后的晚到结果仍受数据库执行栅栏保护。

## 返回边界

所有记录含 `execution_mode: "local_only"`，明确服务端没有网页派发功能。时间戳和连接报价的有效期均为字符串。订阅报价仅返回使用类型、供应商、模型、连接版本、连接 ID 和期限，不以 0 金额代表订阅额度。历史 API 模式只读展示 `kind: "api", available: false`；此接口不能批准其金额同意。

列表仅返回状态、摘要、报价和时间元数据。详情的 `shared_content` 是冻结系统指令与精确用户输入字符串；`candidates` 只含临时编号、标题、订阅 ID 和条目键，供前端对应评分，不返回原始完整候选快照。`scores` 仅含临时编号、分数（允许 null）及理由。取消、到期或失效后这三项为 null。文本必须按不可信纯文本显示，不能当作 HTML 或指令执行。

不返回 OAuth 令牌、host/client/subject、派发标识或原始供应商响应。审计时间戳为字符串，不复制正文。输出字段显式列举，不直接序列化仓储记录。

## 会话与错误

owner 只从会话取得，管理员 Bearer 不能替代用户登录。变更操作还要求 `X-Requested-With: personal-ai`；先检查会话和 CSRF，再处理参数/可选仓储。请求体上限沿用 16 KiB，拒绝未知、重复或类型错误字段；查询参数也使用白名单。处理后的成功和错误响应均设置 `Cache-Control: no-store`。

401 表示未登录或会话失效；403 为 CSRF 拒绝；400 为参数错误；422 为 JSON 字段或类型错误；413 为请求过大；404 为不存在或不属于当前用户；409 为版本/摘要/生命周期冲突；503 为仓储不可用。错误不回传内部数据库诊断。

Next.js 代理只放行上述路径和方法，继续传递原 Cookie/CSRF 头；Nginx 使用现有 `/api/` 代理。没有 `/run`、`/claim`、`/confirm` 或令牌上传入口。已接入[评分授权与结果页面](rss-value-ui.md)，登录用户管理批准后，再通过本机显式命令执行原请求。

## 验收

`make test-subscription-connections` 同时运行新评分 HTTP 用例：用户隔离、精确同意、拒绝伪造价格/身份、失效连接、16 KiB 限制、分页、会话退出、过期清理、已完成评分查询及连接撤销后的清理。普通 `make check` 覆盖关闭仓储时各接口的会话/CSRF 优先级。

完整 `make smoke` 在 Next.js 和 Nginx 下验证真实私有连接与 RSS 候选夹具的预览、批准、列表、查询、取消和审计，并确认网页没有执行路由。没有真实模型调用。此轮不修改页面，不重跑 Playwright；后续页面需补浏览器交互验收。

本轮验证通过：`make check`、前端 lint/typecheck/build，以及完整 `make smoke`（含 162 个 PostgreSQL 测试、新增 3 个评分 HTTP 数据库用例、双网关授权流程和数据库/缓存重启恢复）。本机 Rust 检查使用 1.99.0，生产镜像使用 Rust 1.96；隔离测试容器、网络和数据卷已自动清理。未调用真实订阅模型，未运行可选对象存储/向量扩展验收。
