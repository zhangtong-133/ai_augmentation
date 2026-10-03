# 订阅连接归属与撤销

本阶段把本地 ChatGPT 注册关联到一个明确的应用用户，保存有期限的连接元数据。RSS 评分预览与批准会在同一用户锁内检查连接归属、版本、供应商、模型和期限。撤销、更新或发现连接到期时，相关未完成评分预览在同一事务内失效并清除分享快照。

这仍不是评分执行器。连接记录不是可执行凭据，也不代表真实推理已验证。当前入口是有数据库权限的本地主机管理员命令，没有新增 Web 路由或页面。

## 命令

先运行应用完成 migration `0034_subscription_connections.sql`，再按 [本地订阅接入](chatgpt-local.md)登录。绑定需要导出 `DATABASE_URL`（本机数据库使用 loopback 地址），命令使用 `connect_existing`，不自动迁移。

```bash
# USER_UUID 是已有应用用户；CONNECTION_UUID 是你为本次绑定生成并保存的非零 UUID。
# 首次版本为 0；重新验证/变更使用 connections 输出的当前 revision。
target/debug/chatgpt-connect "$HOME/.config/personal-ai-chatgpt" \
  bind personal USER_UUID CONNECTION_UUID 0

target/debug/chatgpt-connect "$HOME/.config/personal-ai-chatgpt" \
  connections USER_UUID

# 多页时将返回的 next_cursor 作为最后一个参数。
target/debug/chatgpt-connect "$HOME/.config/personal-ai-chatgpt" \
  connections USER_UUID AFTER_UUID

target/debug/chatgpt-connect "$HOME/.config/personal-ai-chatgpt" \
  revoke-binding USER_UUID CONNECTION_UUID REVISION
```

绑定前会核对已登录身份与订阅权限，必要时刷新令牌并落盘，再从官方接口读取模型列表。不会发送推理请求。仅绑定明确指定的应用用户，不能根据邮箱猜测归属。该命令是管理员操作，不能开放给不可信网页输入；管理员须确认本地账户持有人同意关联到该用户。

绑定期限不晚于当前 access token 期限，并限制在一小时内（CLI 预留少量时钟余量）。连接过期后需要显式重新验证和绑定；不会在后台续期。更新模型列表、标签或期限会提升 revision 并使旧评分同意失效。响应丢失时先查询；旧版本不能覆盖后续变更，不能自动复活已撤销连接。

`revoke-binding` 只撤销应用侧关联和评分授权，不撤销 OpenAI 会话；本地 `logout` 只处理 OAuth 会话，不自动定位数据库关联。需要彻底断开时先 `revoke-binding`，再 `logout`，必要时到 [ChatGPT 用量设置](https://chatgpt.com/settings/usage)解除远端授权。

## 存储与事务

- `subscription_connections` 保存用户/连接 UUID、主机 ID、client ID、subject 的 SHA-256 摘要、标签、模型目录、版本、状态与期限。令牌、邮箱和明文 subject 不进入数据库。
- 查询输出仅含连接 ID、标签、版本、状态、模型和期限，不返回注册身份。详情、列表、撤销和评分校验均包含 owner 条件。
- `(host_id, client_id)` 全局唯一，已绑定记录不能修改 owner、主机、client 或 subject。即使撤销，也不允许将同一注册直接转给其他用户；用户删除按已有级联清理。
- 每用户最多保留 20 个连接，撤销不返还创建名额，列表按 UUID 每页 10 条。每连接最多 998 个活跃版本，保留后续到期/撤销版本；审计规模有界。
- `subscription_connection_audit` 记录版本、状态和数据库时间。保存与撤销使用用户行锁，和 RSS 预览/批准共用串行化边界。数据库触发器同时作废该连接的 draft/authorized 评分记录；审计失败会回滚连接和预览变化。
- 到期按访问惰性处理。仓储所有管理操作与评分读取都会检查期限；失败事务可能回滚清理，但后续成功读取继续清理，批准仍拒绝失效连接。
- 新迁移将历史未绑定的订阅评分预览设为 invalidated，清除其分享快照并保留审计；API 金额模式不受影响。
- `VerifiedSubscriptionConnection` 是内部可信运行时输入，不能从 HTTP JSON 反序列化。仓储校验结构与归属，OAuth 验证仍由接入层完成。

## 后续执行边界

本地主机文件仍是凭据的唯一来源。执行器必须重新核对主机/client/subject、最新令牌权限、连接版本和用户同意，并在发送前处理撤销竞争；不能仅凭 `active` 元数据发起调用。OpenAI 侧权限和模型可用性可能在登记后变化，模型列表也不是推理验收。后续需补齐用户会话下的管理入口、执行领取及 Web 页面。

账户隔离及授权分离遵循 [官方 OpenAI documentation：账户与会话](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)。

## 验证

一次性 PostgreSQL 测试覆盖并发绑定幂等、跨用户读写拒绝、身份不可替换、旧操作不能复活、撤销重放、版本审计、重连持久化、分页/配额/到期，以及评分批准与撤销竞争、无效报价和审计失败回滚。真实订阅账户、模型推理和网页操作不在本轮验收范围。
