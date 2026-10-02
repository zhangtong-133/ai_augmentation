# GitTool：本地只读提交历史

新增固定工具 `git_log`，通过现有 `POST /api/tools/git_log` 查询管理员为当前用户授权的本地仓库最近提交。默认未注册，不访问远程、不调用模型，也不修改仓库；与 FileReader、knowledge_search 共享每日工具调用额度和一次性审计。

## 部署与授权

API 启动时读取 `GIT_REPOSITORIES_JSON`，未设置或 `[]` 禁用。配置示例：

```json
[{"owner_id":"用户UUID","repository_id":"project","path":"/srv/repositories/project"}]
```

配置最多 32 条、64 KiB，拒绝额外字段、重复用户/别名、非法 UUID 和无效路径；错误只返回通用说明，不打印配置。每条映射独立授权一个用户，不从请求体或模型取得 owner_id。不同用户可以使用同名别名，实际目录由部署配置绑定。别名为 1–64 个 ASCII 字母、数字、下划线或连字符。

当前支持无 promisor/partial-clone 配置的普通本地仓库，路径必须绝对，包含 `.git/HEAD` 与 `.git/objects`；启动时固定规范化后的 Git 目录。裸仓库和 `.git` 为文件的 worktree 不支持。管理员应提供可信、本地、可读的仓库，并以只读挂载限制进程权限；容器内路径必须与配置一致。不要把来自用户上传的任意 Git 目录当作可信配置。仓库更新由部署者独立完成，不由 API 自动 fetch。

API 生产镜像包含 `/usr/bin/git`，Compose 透传配置，但不会挂载宿主仓库。需要自行在部署 override 中添加只读挂载，并确保 API 运行用户可以读取。更改映射需重启 API；配置绑定不写入 PostgreSQL，无数据库迁移。

## HTTP 协议

```json
{"repository_id":"project","limit":10}
```

limit 默认 10，范围 1–20。拒绝路径、命令、修订表达式、其他字段或非法别名。要求用户会话、CSRF 头及 UUID `Idempotency-Key`；管理令牌不能代替会话。未知或属于他人的仓库统一拒绝为 `tool_denied`，不泄露服务器路径或仓库内容。

成功响应遵循现有工具协议，output 包含 repository_id 和 commits。每条提交包含完整 commit ID、十进制字符串 committed_at_unix_seconds、subject 和 subject_truncated。提交从本地 HEAD 开始，标题最多 1000 个 Unicode 字符；不返回作者邮箱、正文、文件内容、差异或远程地址。标题是不可信文本，消费端不得当作指令或 HTML 执行。没有有效 HEAD 的空仓库、对象缺失或进程故障返回通用工具失败。

manifest 标记只读且无供应商费用；manifest 不公开仓库映射。授权失败及执行失败按现有工具规则计入尝试额度，格式校验失败不启动执行。重复 Idempotency-Key 只查询已用状态并返回冲突，不重复运行 Git、不持久化或重放提交正文。MCP 凭据和固定/模型 Agent 计划仍只具备原有知识检索范围，未扩展到仓库。

## 执行边界

使用独立 `git-local` 适配器，直接启动固定 `/usr/bin/git`，不经过 shell。历史读取命令为固定选项的 log，执行前仅增加固定 config 查询，输入只影响已验证的数量及预先授权目录；不允许 Git 选项、路径或 ref 插值。

子进程清空继承环境，禁用全局/系统配置、pager、签名显示、hooks、fsmonitor、replace 对象和交互认证，关闭可选写锁；以空 `GIT_ALLOW_PROTOCOL` 禁用所有传输协议，并设置 `GIT_NO_LAZY_FETCH`。执行前通过固定 config 查询拒绝任何 promisor/partial-clone 配置（包括设为 false 的 promisor 项），兼容旧 Git 不识别 GIT_NO_LAZY_FETCH 的情况。仓库配置仍用于读取本地仓库格式，因此部署仓库必须可信。固定格式不输出 diff，也不运行外部 diff。

stdout 至多读取 64 KiB 加一个溢出探测字节，超限整体失败；stderr 丢弃，错误不包含 Git 输出或路径。5 秒超时终止等待，child 的 kill_on_drop 负责结束进程；现有工具执行器还限制并发为 2 并校验最终输出大小。启动后管理员移动或修改目录可能导致本次请求失败，工具不会自动重试。

参数和环境边界参照 [git-log 官方文档](https://git-scm.com/docs/git-log)及 [Git 环境变量说明](https://git-scm.com/docs/git)。本轮只交付最近提交查询，未提供文件差异、分支操作、提交、推送或远程仓库接入。

## 验证

真实临时仓库测试覆盖 Unicode 提交、数量限制、用户隔离、引用不变、无写锁、标题截断、超大输出、空仓库失败及 promisor 配置拒绝且不写 FETCH_HEAD；配置/参数测试覆盖重复绑定、禁用配置、路径和命令注入拒绝。Tower HTTP 验证会话、CSRF、no-store、额度、一次性执行、未知仓库拒绝与正文不进入审计。临时仓库随测试清理。

2026-10-02 验收：Rust 1.99 `make check`、前端 lint/typecheck/build、Compose 配置及完整 `make smoke` 通过，包含 135 项 PostgreSQL 测试、双入口 HTTP 和重启/缓存故障恢复。最终 API 生产镜像构建通过；在断网、只读根文件系统、仅临时目录可写的该镜像中运行 3 项真实仓库测试及 1 项 GitTool HTTP 测试，全部通过。临时服务与数据已清理。本轮没有页面变更，未重跑 Playwright、对象存储或向量索引专项，也未访问实际用户仓库或远程 Git 服务。
