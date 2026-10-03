# ChatGPT 订阅本地接入

本阶段提供 `chatgpt-connect` 本地命令：登录、查看连接、列出账户可用模型、显式单次调用、退出。它使用独立 OAuth 凭据，不读取 Codex 或 ChatGPT 桌面端的登录文件，不需要 `OPENAI_API_KEY`，不写入 API 金额账本。已提供[本地显式用户连接绑定](subscription-connections.md)，网页和 RSS 评分执行器仍待接入；命令成功不代表已有网页模型入口已经使用订阅。

## 使用

在运行浏览器的同一台 Unix 主机上执行，不放进 Docker 或远程 SSH 会话。先编译：

```bash
cargo build -p api-server --bin chatgpt-connect
mkdir -p "$HOME/.config"
target/debug/chatgpt-connect "$HOME/.config/personal-ai-chatgpt" login personal
```

终端显示 **Continue with ChatGPT** 和登录链接，手动在本机浏览器打开。查看 OpenAI 的权限说明，决定是否允许订阅用量。工具在 `127.0.0.1` 的临时端口等待回调，五分钟后超时。登录不会发起模型调用。

```bash
target/debug/chatgpt-connect "$HOME/.config/personal-ai-chatgpt" status
target/debug/chatgpt-connect "$HOME/.config/personal-ai-chatgpt" models personal
```

`personal` 是本地账户标签，只允许字母、数字、下划线和连字符。不同账户或工作区使用不同标签；返回同一账户应复用原标签。选用 `models` 输出的实际 model 值，替换下方 `MODEL`：

```bash
printf '请简短回答：连接成功。\n' | target/debug/chatgpt-connect \
  "$HOME/.config/personal-ai-chatgpt" ask personal MODEL --use-subscription
```

`--use-subscription` 表示同意把标准输入发送到 OpenAI 并使用所选账户的订阅额度，或使用你在 ChatGPT 设置中允许的 credits。省略该选项不会发送请求。输入上限 32 KiB；不自动附加知识库、RSS、历史消息或文件。工具先刷新该账户模型列表，再发送一次推理请求，完整结束后显示文本；失败不输出半截结果，也不自动重发。

```bash
target/debug/chatgpt-connect "$HOME/.config/personal-ai-chatgpt" logout personal
```

退出尝试撤销远端会话并清除本地凭据，保留注册 ID 以便再次登录。撤销失败仍清除本地凭据，命令以非零状态提醒你前往 [ChatGPT 用量设置](https://chatgpt.com/settings/usage)解除授权。不要把凭据目录放进 Git、共享目录或云盘。

## 实现与边界

- `llm-openai::chatgpt` 负责供应商协议，应用命令负责本地文件和回调。服务端正常启动不会调用它，没有新增环境开关或数据库迁移。
- 首次保存稳定 UUID host ID；各标签分开保存注册和凭据。新登录成功验证后才替换旧凭据。新注册的 client ID 在换取令牌前落盘，换码失败后可以复用注册再登录。
- 每次登录生成新 state、nonce 和 PKCE S256；检查重复回调字段、state、拒绝授权、返回 client ID 和五分钟有效期。回调只消费一次。固定 loopback 路径 `/auth/callback`。
- 通过官方 OIDC discovery/JWKS 验证 RS256 签名、issuer、audience、到期时间、nonce 和 subject；重新登录必须匹配已保存账户。多 audience 时要求正确 azp。可选 ID-token 登录提示被省略，终端授权 URL 不包含 ID token。
- 私有目录 `0700`、文件 `0600`，拒绝最终路径符号链接、硬链接和开放权限；临时文件写入、同步、rename 原子替换。父目录必须由用户信任。持有文件锁直到命令结束，同一凭据目录不能并行刷新/调用/退出。锁随进程结束释放。
- 临近过期先刷新并持久化整个凭据集合，再调用模型；使用更新后的权限，拒绝缺失订阅或 resource 授权。刷新没有自动重试，未知结果或失效令牌可能需要重新登录。
- 固定官方端点，禁用环境代理（需要直连官方服务），禁止重定向、推理重试和 API Key 回退。HTTP 总超时 90 秒，普通响应 1 MiB、单个 SSE 事件 1 MiB、累计流 4 MiB、正文输出 128 KiB；支持分片 UTF-8 和 LF/CRLF/CR。只有合法 `response.completed` 才返回成功，额度不足、failed、incomplete 和断流均失败。
- 本地输出/接收限制不是远端生成 token 上限，也不构成费用上界。订阅通道与 API 金额授权保持独立。
- 错误只包含固定诊断，不打印供应商响应、回调 code 或凭据；账户/模型元数据以 JSON 转义输出，答案过滤终端控制字符。
- 当前凭据属于执行命令的本地 OS 用户，可用独立的 `bind` 命令显式绑定应用用户，详见[连接归属与撤销](subscription-connections.md)。这个文件仍不是多用户 Web 凭据仓储。

## 验证

本地夹具覆盖 JWT 签名/身份错误、PKCE 和 client 替换、订阅权限、刷新轮换、流式成功/断流/额度失败、重定向禁止、一次发送、文件权限/链接/锁及回调重放。测试 RSA 密钥是公开生成夹具，不能用于真实身份。

```bash
cargo test -p personal-ai-llm-openai chatgpt
cargo test -p api-server --bin chatgpt-connect
```

没有真实 Pro 登录或真实推理验收：需要用户自行浏览器授权，且只有成功结束的一次调用才能验证该账户当时可用。

## 官方依据（2026-10-03 核对）

- [OpenAI：注册与登录](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
- [OpenAI：账户与会话](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)
- [OpenAI：模型与推理](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)
- [OpenAI：预览限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)

可用性取决于实际账户与授权。该入口适用于符合条件的开源/本地项目，不能据此假设任意商业托管部署均已获支持。
