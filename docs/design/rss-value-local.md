# RSS 订阅评分本地命令

`chatgpt-connect` 现已把[订阅评分执行器](rss-value-execution.md)接到本地 ChatGPT 运行时，提供预览、精确批准、单独执行、查询及取消命令。本地命令不新增迁移或后台扫描；现已提供[用户评分 HTTP](rss-value-http.md)，已接入[评分授权与结果页面](rss-value-ui.md)。日报继续使用原规则分数。

## 本地操作流程

这是具有数据库访问权限的本机管理员工具，不是用户会话 API。操作者必须有权代表指定应用用户管理分享与订阅使用同意。`USER_UUID` 不能作为身份认证材料公开给浏览器；部署时不要把这个命令封装成接受任意用户 ID 的公共接口。

先按[本地接入说明](chatgpt-local.md)完成所选 LABEL 的登录和显式 `bind`。本地进程导出已有 `DATABASE_URL`，应用数据库需已运行至 `0035` 迁移；工具使用 `connect_existing`，不隐式迁移。数据库本地连接使用 loopback 地址。

```bash
cargo build -p api-server --bin chatgpt-connect
# USER_UUID 是应用账户；CONNECTION_UUID 是已绑定连接；REQUEST_UUID 是本次新生成的 UUID。
target/debug/chatgpt-connect "$HOME/.config/personal-ai-chatgpt" \
  value-preview USER_UUID CONNECTION_UUID MODEL REQUEST_UUID
```

预览从数据库读取候选和偏好，冻结具体模型及连接版本，不打开凭据文件、不访问 OpenAI。JSON 输出中的 `shared_content` 是将要发送的系统指令及用户输入内容；`local_candidate_mapping` 只在本机将临时编号对应回原条目，不整份发送给模型。`pricing` 为 subscription，不使用零美元代表订阅额度。输出保留原 digest、有效期和历史状态；同一个请求 ID 不换价、不换模型、不重建候选。

审阅模型、分享内容、连接和用量说明后，复制该预览的完整摘要到独立批准命令：

```bash
target/debug/chatgpt-connect "$HOME/.config/personal-ai-chatgpt" \
  value-approve USER_UUID REQUEST_UUID DIGEST --share-content --use-subscription
```

两项确认都必须显式提供。该操作只保存同意，不登录、不刷新令牌、不请求模型。变更内容、连接或偏好后必须重新预览，不能自动接受新 digest。

批准后，在有效期内单独执行：

```bash
target/debug/chatgpt-connect "$HOME/.config/personal-ai-chatgpt" \
  value-run personal USER_UUID REQUEST_UUID --use-subscription
```

只有此命令可能发起推理并消耗所选账户的订阅额度，或账户设置允许的 credits。模型固定为预览中的模型，不能在执行时替换。先读取原状态；非 authorized 状态直接显示，不读取凭据。需要刷新时，在领取前刷新并持久化令牌，然后持有整个凭据目录锁直至执行及写回结束。运行时借用同一账户，不在验证与发送之间换号；仓储仍匹配 host/client/subject、连接版本及当前模型权限。

推理完成后输出严格校验的评分或放弃评分。重复运行成功记录只返回已有结果，不重发。draft、running、unknown、cancelled 等未完成状态会输出原状态，并以非零退出码提醒查询。身份复核或存储失败可能留下 running，待原执行期限后通过读取回收为 unknown；不能重置原请求或自动重派。

```bash
target/debug/chatgpt-connect "$HOME/.config/personal-ai-chatgpt" value-show USER_UUID REQUEST_UUID
target/debug/chatgpt-connect "$HOME/.config/personal-ai-chatgpt" value-cancel USER_UUID REQUEST_UUID
```

查询、取消不持有凭据锁，可在另一终端对运行中的请求操作。取消取得发送许可之前会阻止发送；已经获准的外部请求可能继续消耗用量，晚到结果会丢弃。取消、过期或失效后的正文按执行仓储规则清除。进程中断或未知结果时先查原请求，不自动生成新 ID。

## 传输与验证

按 2026-10-03 核对的 [OpenAI 订阅请求限制](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)，评分使用 `instructions` 携带冻结的系统指令，`input` 数组仅携带用户消息，并设置 `store:false`、`stream:true`。不发送 `temperature`、`max_output_tokens`、工具或历史会话标识。订阅发送方法与原 `ask` 共用受限 SSE 解析及禁止重试的固定端点 HTTP 客户端。

评分输入总量最多 64 KiB，完整文本结果最多 32 KiB，并经过原评分协议校验；本地限制不构成供应商 token 或费用硬上限。不存在 API Key 回退。诊断为固定文本，预览/评分通过 JSON 转义输出，不展示 OAuth 令牌、注册标识或账号证据。

`make check` 覆盖严格命令参数、私有文件锁、权限与模型拒绝、请求字段、流式完成、额度失败不重试。`make smoke` 新增真实 PostgreSQL 的 CLI 管理流程与锁定账户运行时夹具验收：预览/批准不推理、错误摘要拒绝、一次执行、重复调用不重发、查询隔离和取消后不能批准。传输和数据库分别使用确定性夹具；没有使用真实 Pro 凭据或真实模型，本轮不宣称具体账户已验证可用。

已提供用户会话下的[评分授权/结果 HTTP](rss-value-http.md)，已接入[用户页面](rss-value-ui.md)；多用户服务器不能直接复用本机管理员权限或共享任意人的凭据文件。

本轮验收：Rust 1.99 `make check`、前端 lint/typecheck/build、完整 `make smoke` 通过；包含 162 项 PostgreSQL 测试及新增的本地评分管理流程测试，生产 API 使用 Rust 1.96 构建通过。无页面改动，未重跑 Playwright；未运行对象存储/向量专项或真实 Pro 推理。测试容器、网络和临时数据已清理。
