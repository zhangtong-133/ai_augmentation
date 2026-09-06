# Sprint 1：登录会话与 Dashboard

升级说明：知识库迭代将 Cookie 改为 personal_ai_session_v2、Path=/api，需重新登录，见 [Markdown 设计](sprint-2-markdown.md)。下文记录首次会话实现。

本轮承接 API/用户持久化迭代。用户先由管理员创建并设置密码，再通过邮箱与密码登录；不开放公共注册。原部署管理 Token 继续仅用于管理接口。

## 接口与权限

| 接口 | 权限 | 行为 |
|---|---|---|
| POST /api/users/{id}/password | 管理 Bearer Token | 设置/重设 12–256 字节密码；原会话全部撤销 |
| POST /api/auth/login | 无会话，需 X-Requested-With: personal-ai | 邮箱密码登录，返回 HttpOnly Cookie |
| GET /api/auth/me | 有效会话 Cookie | 返回当前用户 id、email、display_name |
| POST /api/auth/logout | 需 X-Requested-With: personal-ai | 撤销当前会话并清除 Cookie；无 Cookie 时幂等成功 |

密码使用 Argon2id 默认参数和随机盐，哈希工作在 blocking 线程池执行。登录失败统一 401；进程全局每分钟最多 20 次尝试，超限 429。这是个人单实例的基础限制，重启会重置；多实例部署需共享限流存储。

会话原始 token 由两个随机 UUID v4 拼接产生，仅放入 Cookie；数据库仅保存 SHA-256 摘要。会话有效期固定 8 小时、不滑动续期，数据库时间决定是否过期。创建新会话时清理过期记录。密码重设和旧会话撤销在同一事务中完成，登录创建会话再次锁定用户并检查哈希未改变，避免密码重设竞态。

Cookie 属性：HttpOnly、SameSite=Strict、Path=/api/auth、Max-Age=28800，默认 Secure。仅本地 HTTP 开发显式配置 SESSION_COOKIE_SECURE=false；HTTPS 使用 true。浏览器不保存管理 Token，也不使用 localStorage 保存会话。

跨站防护使用自定义请求头与 SameSite Cookie，不开放 CORS。不要在反向代理添加允许任意 Origin 携带凭证/自定义头的 CORS 配置。认证响应标记 no-store，Service Worker 不处理 /api/ 请求。

## 数据与适配器

0003_auth.sql 新增 users.password_hash 和 sessions 表（摘要主键、用户外键、过期时间与索引）。旧用户初始没有密码，必须先设置。存储端口新增密码查询/设置、会话创建/读取/撤销方法；PostgreSQL 实现在适配器内，API 不直接调用 SQL。

## Dashboard 与代理

AccountPanel 首次加载并行请求 readyz 与 auth/me。显示服务不可用、未登录、已登录及提交中状态；错误凭证、限流和网络故障有单独提示。刷新通过 Cookie 恢复身份；退出失败时保留账户视图并提示重试。

Next.js 提供 /api/[...path] 的受限代理，仅转发健康、就绪及认证接口，不代理用户管理接口。API_INTERNAL_URL 默认为 http://127.0.0.1:8080，Compose 显式设为 http://api-server:8080。它只在服务器读取。Nginx 网关的 /api/ 仍直接转发 Rust API，浏览器使用相对地址，两种访问入口均为同源。

## 首个账户的初始化

1. 启动 PostgreSQL，在 API 环境设置 DATABASE_URL、API_AUTH_TOKEN；本地 HTTP 还需 SESSION_COOKIE_SECURE=false。启动 cargo run -p api-server，迁移自动执行。
2. 按上一轮文档通过 POST /api/users 创建用户，记录返回的 UUID。
3. 用管理 Token 设置密码：

```bash
curl --noproxy '*' http://127.0.0.1:8080/api/users/替换为用户UUID/password \
  -H "Authorization: Bearer $API_AUTH_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"password":"替换为自己的长密码"}'
```

4. 启动 npm --prefix apps/web run dev，在 localhost:3000 输入邮箱和密码。通过 Compose 运行则在 .env 设置相应配置。

尚未实现：找回密码邮件、自助修改密码、MFA、多租户授权、学习任务和今日报告业务数据。忘记密码可由管理员调用设置密码接口重置。

## 验证

HTTP 测试覆盖管理员设置密码、错误密码、缺少 CSRF 请求头、登录 Cookie、当前身份、退出撤销、重设密码撤销旧会话与旧密码拒绝，以及全局尝试限制。PostgreSQL 集成测试增加会话持久化、撤销、过期和旧凭证拒绝检查，需 TEST_DATABASE_URL 指向可丢弃数据库。
