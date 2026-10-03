# 订阅连接用户管理 HTTP

在[连接归属与撤销](subscription-connections.md)之上提供登录用户自己的连接列表、详情和撤销入口。接口只管理已由本地可信运行时绑定的元数据，不接受 OAuth 令牌、账号声明、owner 或创建/续期请求，不发起模型调用。

## 接口

| 方法与路径 | 请求 | 行为 |
| --- | --- | --- |
| `GET /api/subscription-connections` | 可选 `after=UUID` | 按 UUID 升序，每页最多 10 个，返回 `items` 和 `next_cursor` |
| `GET /api/subscription-connections/{id}` | 无查询参数 | 读取当前用户的一条连接 |
| `POST /api/subscription-connections/{id}/revoke` | `{"revision":"1"}` | 精确版本撤销，返回当前元数据 |

列表和详情只返回 `id`、`label`、`revision`、`status`、`models`、`valid_until_unix_ms`。版本与毫秒时间以十进制字符串传输，不丢失 JavaScript 整数精度。响应不包含主机 ID、client ID、subject、邮箱或任何令牌。

所有接口使用现有登录 Cookie。撤销还要求 `X-Requested-With: personal-ai`；不能用管理员 Bearer token 替代用户会话。owner 只来自服务器认证结果，跨用户详情/撤销返回 404。请求体、列表游标以及详情/撤销的查询参数均严格校验；多余字段（包括 `user_id` 和 `access_token`）被拒绝。

撤销版本必须是 `1..999` 的规范十进制字符串。旧版本与当前状态不匹配返回 409，客户端应重新读取并确认，不能自动使用新版本再次撤销。原撤销请求重试保留仓储既有幂等行为，不增加版本或重复审计。

状态和期限由仓储复核，读取会惰性处理到期。撤销、到期触发关联评分预览失效的事务逻辑沿用上一阶段。应用侧撤销不会退出 ChatGPT 账户；本地 `logout` 和远端解除授权仍是独立操作。

## 错误与部署

- 401：缺少或失效会话；403：CSRF 失败。
- 400：非法 ID、游标、版本或查询参数。
- 422：JSON 结构/类型不符合协议；413：超过现有 16 KiB 请求体限制。
- 404：当前用户找不到连接；409：版本或状态冲突。
- 503：仓储缺失或暂时不可用。

接口响应设置 `Cache-Control: no-store`，错误不回传存储内部信息。API 启动时使用现有 PostgreSQL 仓储，无新增环境变量、数据库迁移或模型执行开关。连接登记仍使用管理员本地 `bind` 命令；本次没有创建、重新绑定或 OAuth 浏览器回调接口。

Next.js 代理仅允许上表方法和路径，继续原样传递 Cookie/CSRF 头；Nginx 沿用现有 `/api/` 转发。已接入[连接管理页面](subscription-connections-ui.md)，支持列表、详情、明确撤销和异常结果核对。

## 验证

`make test-subscription-connections` 在一次性 `TEST_DATABASE_URL` 上验证用户隔离、拒绝上传凭据、未知字段、字符串版本、分页、到期冲突、原请求重试和应用会话退出；普通 `make check` 验证仓储关闭时的会话/CSRF 优先级。

`make smoke` 已纳入这些 HTTP 测试，并通过 Next.js/Nginx 两个入口验证元数据白名单、会话、CSRF、跨用户拒绝及幂等撤销。夹具只插入测试元数据，不需要真实 Pro 账户或模型请求。没有页面改动，浏览器交互验收留待页面阶段。
