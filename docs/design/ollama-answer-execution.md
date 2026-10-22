# macOS Ollama 私有问答单次执行

2026-10-22：将 macOS ARM64 上已安装的 Ollama 接入[现有执行闭环](answer-execution.md)。本批交付运行器适配、精确同意、资源保护和持久化恢复，用确定性服务验收工程行为。已有 Qwen 合成实验的成绩与失败保留，质量门槛不控制此功能的开发或部署开关。

## 用户流程与授权边界

检索并选择资料后，页面可选择 `llama.cpp` 或 `Ollama（macOS）`。Ollama 预设地址为 `http://127.0.0.1:11434`、模型为 `qwen3.5:9b`；用户可以在新建预览前修改。选择预设只修改新预览输入，既有授权仍显示和绑定原完整请求，不自动取消、确认或执行。

用户先创建完整请求预览，核对目标、模型、原文、协议与指纹，再勾选分享材料和使用本地计算资源两个同意项。保存授权后须另行点击“执行一次”；保存、打开详情、刷新、重新登录和恢复结果都不发送模型请求。未领取的授权可以取消；运行期间可以取消等待；成功后可删除本次结果。失败、取消、超时或结果未知不恢复授权，也不自动重试。

准备请求使用 `backend: "ollama"`，服务端保存此选择。省略 `backend` 或显式 `local` 继续表示旧 local 后端；序列化旧默认时不额外加入字段，保留已有意图摘要。迁移 `0044_answer_authorization_backend.sql` 将旧授权明确归为 `local`，不重写旧请求或指纹。Ollama 原生请求与旧 `local-knowledge-answer-v2` 授权不可互换，切换 API 的后端、目标或模型后须重新预览并确认。授权绑定模型别名和完整请求；权重 SHA 由进程内准入固定，不是跨 API 重启持久化的权重同意。

## 原生请求

新 profile 为 `ollama-local-answer-v1`，使用单次 `POST /api/chat` 和有界 NDJSON 流。预览与传输复用同一构造器，指纹包含 profile、目标 URL、完整发送正文及问答协议摘要。每个授权最多领取和发送一次，不采用历史合成候选的多阶段请求或内部重试。

完整预览包含原有系统提示、问题和选中原文，以及以下固定原生参数：

| 字段 | 值 |
|---|---|
| `runner` | `llamacpp` |
| `format` | 现有 `answer` / `citations` / `insufficient_evidence` 问答 JSON schema |
| `stream` / `think` | `true` / `false` |
| `keep_alive` | `30` 秒 |
| `truncate` / `shift` | 均为 `false` |
| `options.num_ctx` / `options.num_predict` | `8192` / `2048` |
| `options.temperature` / `options.seed` | 均为 `0` |
| `options.top_k` / `options.top_p` / `options.repeat_penalty` | 均为 `1` |
| `options.presence_penalty` / `options.frequency_penalty` | 均为 `0` |

传输要求完整终态、合法协议和生产精确引用校验。模型自述、合法 JSON 或逐字引用均不证明结论正确；页面继续提示用户核对。响应、材料或引用非法时丢弃结果，不以部分文本作为成功答案。

## 身份与统一内存保护

专用 `scripts/ollama-answer-admission.mjs` 只读取本机运行器和模型元数据、驻留状态及统一内存估计，不加载、下载、卸载或推理。检查子进程只接收检查模式、固定 endpoint 和 model，不接收问题、私有材料、授权、会话或订阅凭据。

API 在领取前及实际发送前运行 `answer-admit`；原生执行期间每 2 秒和返回结果前运行同一脚本的 `answer-monitor`。单次检查上限 10 秒，脚本总截止 7 秒。首次成功准入的身份摘要固定在 API 进程内；后续模型或运行器身份变化拒绝新执行，运行期间变化则停止等待并拒绝保存答案，部署者需显式重启 API 才能重新建立身份。

准入至少保留 6 GiB 加 512 MiB 的统一内存估计余量。模型未驻留时另外计入 1.5 倍模型大小；复用同一模型、运行器和上下文时另外计入 1 GiB 推理缓冲。在途 monitor 只检查 6 GiB 加 512 MiB 的基础余量，不重复扣除已加载模型或推理缓冲。无法确认元数据、驻留冲突、基准评估占用或预算不足均拒绝准入。用户授权的真实执行可以由 Ollama 冷加载已有模型；检查脚本本身不触发该加载。

本 API 进程一次只执行一个请求，但不能保证共享 Ollama 没有其他客户端。取消、资源失效和超时只停止本次等待并抑制晚到结果，不停止共享 Ollama 进程或发送卸载请求。`keep_alive=30` 限制本请求要求的空闲驻留时间，外部使用仍可能延长驻留；统一内存估计与周期检查也不构成外部排他或内存硬配额。

会话、CSRF、当前所有者、来源版本、单次领取、发送审计及结果读取校验继续沿用原闭环。取消、退出、来源变化或失败后不保存晚到正文；历史列表只显示元数据，结果未知时仅查询原请求。

## 显式部署

前提是同一台 macOS ARM64 主机已有正在运行的 Ollama 和所选模型；Ollama 最低版本为 0.40.1，且必须暴露所需的运行器、模型身份和 think 能力元数据。部署流程不自动安装或拉取。为 native API 显式导出以下变量，并继续提供原有数据库、认证等配置：

```bash
export LOCAL_ANSWER_EXECUTION_ENABLED=true
export LOCAL_ANSWER_BACKEND=ollama
export LOCAL_ANSWER_ENDPOINT=http://127.0.0.1:11434
export LOCAL_ANSWER_MODEL=qwen3.5:9b
export LOCAL_ANSWER_GUARD_SCRIPT="$PWD/scripts/ollama-answer-admission.mjs"
# 可选：指定 Node 可执行文件绝对路径；默认使用 node。
# export LOCAL_ANSWER_NODE_BINARY=/absolute/path/to/node
cargo run -p api-server
```

API 不自动读取 `.env`。默认 `LOCAL_ANSWER_EXECUTION_ENABLED=false`，默认后端仍为 `local`；仅配置后端不会启用执行。目标限制为固定回环 HTTP，页面预设不代替管理员配置。标准 Compose API 镜像没有宿主运行器、Node 和资源检查环境，不透传这些变量；Web 与数据服务可继续使用 Compose。配置细节见[环境说明](../ENVIRONMENT.md)。

## 本批验收

确定性验收覆盖完整原生发送体与预览一致、旧摘要兼容、后端持久化、旧授权拒绝换运行器、独立确认与单次执行、NDJSON 终态及引用、准入和在途检查失败、身份变化、取消/来源失效、结果恢复和删除。页面覆盖两种运行器的真实预览与默认关闭行为，并以契约夹具验证 Ollama 确认、执行和恢复，以及切换预设不修改旧授权；截图包含 `ollama-answer-preview.png`。

本批已通过 `make check`（rustfmt、Clippy `-D warnings`、376 项工作区测试），以及前端 lint、typecheck、生产 build 和 16 项单测。新增准入脚本 10 项测试、既有 Ollama 元数据/资源回归 24 项和 Linux 监督预算 6 项均通过；新增准入测试已接入 CI。隔离 PostgreSQL、Redis、Qdrant 与 HTTP 服务集成 302 项通过，其中 PostgreSQL 187 项，授权/执行 HTTP 7 项覆盖两种后端。

`BROWSER_SPEC=answer-authorizations.spec.mjs make browser-test-index` 最终 22 项浏览器测试通过（Next.js 桌面与 Nginx 移动入口各 11 项）；Ollama 预览截图已逐一检查，协议、完整请求、双重确认与移动换行展示正常。隔离栈的容器、网络和测试数据已清理，构建镜像保留。浏览器首轮暴露的运行器下拉框精确标签定位已改为按 combobox 可访问名称定位，重跑通过。

本机 macOS ARM64 的只读 `inspect` 检查通过：Ollama 0.40.7、已有 `qwen3.5:9b`、32 GiB 统一内存，冷驻留时身份与预算均确认；检查未触发模型加载或推理。

未发送真实推理请求，未重跑 Qwen 质量门槛，未使用个人私有原文或付费模型；本轮未运行 MinIO 对象栈、公网导入或其他页面的全量浏览器用例。真实任务的模型质量、延迟及资源峰值仍需按所选模型独立观察；[历史驻留实验](ollama-answer-residency.md)与其他候选报告保持原值，不合并为本适配的质量证明。
