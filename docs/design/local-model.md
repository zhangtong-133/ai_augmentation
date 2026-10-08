# 独立本地模型与游戏显存预算

本批按三阶段验收、提交和推送：① 供应商无关本地推理端口和独立本地聊天 SSE 适配；② 精确地址/模型/分享内容授权、一次执行和现有私有页面观察；③ 项目私有部署、显存预算和真实模型兼容性验收。

2026-10-08 补充：按用户要求新增 [macOS 已安装 Ollama 的独立合成问答评估](ollama-answer-quality.md)。该入口固定运行器与候选请求，使用 Apple 统一内存估算保护，不替换本文的 Linux/NVIDIA 部署，也尚未接入学习/RSS 或私有问答执行。

## 推理边界

`llm::local::LocalTarget` 仅接受规范 HTTP loopback 地址与明确模型标签，不接收凭据、路径、代理或云模型标签。`llm-local` 独立实现 `/v1/chat/completions` 的受限 SSE profile，禁用代理/重定向/工具/thinking，设置 JSON、最多 2048 输出 token。部署机固定 8192 上下文、单模型/单并发。授权与推理后端分离，后续可换 vLLM；仍须单独验收真实协议和资源预算。完整提示最大 5632 UTF-8 字节，保守预留角色模板和输出空间；超限在发送前报错，不截断用户材料。适配器不保证其他服务兼容。

SSE 逐字节拆包，限制单行 64 KiB、传输 256 KiB、文本 24 KiB。绑定返回模型、单候选与 assistant 角色，仅 `finish_reason=stop` 且随后 `[DONE]` 可候选完成。必须等正常 HTTP EOF；截断、长度终止、thinking/工具、错误、终态后内容和非法 UTF-8 不返回成功。网络 60 秒限时，不重试推理；取消 future 关闭本地读取，已送请求不保证服务立即停止。临时正文不是已验证建议，仍需领域校验、来源复核与持久化。

## 显存约束

用户选择与游戏并行，至少预留 6 GiB。2026-10-04 本机实测 RTX 5070 Ti：16303 MiB 总显存、驱动 591.86。使用 Qwen3 4B Q4_K_M（Qwen 官方 GGUF 文件约 2.5 GB），单模型、单并发、限制上下文；模型文件大小不等于实际显存。

部署须先核实 GPU，再测实际峰值。软件预算不是 GPU 硬分区：其他进程和游戏随时可增加占用。启动/发送前检查可用量，运行监控发现不足时停止本项目模型服务；无法查询显存则拒绝启动 GPU 推理。保留显式停止入口，不自动拉起或后台重发。

冷启动或模型已睡眠时要求至少 10752 MiB 可用（6 GiB 游戏余量 + 512 MiB 缓冲 + 4 GiB 加载预算）；模型驻留时发送要求至少 7680 MiB。监督进程每 500 ms 查询一次，查询最多等待 2 秒；低于 6656 MiB、查询失败或 GPU 身份改变时停止本项目进程组，SIGTERM 后最多等待 2 秒再 SIGKILL。游戏占用突增可能在采样和退出间短暂突破余量，无法承诺硬隔离。30 秒没有请求后 llama-server 自动卸载模型，后续显式请求才重新加载。

## 本机部署与使用

固定版本与 SHA-256 见 `infra/local-model-runtime.json`：官方 llama.cpp b11382 CUDA 12.8 发布包、Qwen 官方 GGUF 固定 revision。当前安装入口支持 Linux/WSL2 x86_64，需要 NVIDIA 驱动、Python 3.10+（支持 tar 数据过滤）、curl、dpkg-deb、Node.js 和项目 Rust 工具链。模型及运行库放在 Git/Docker 均忽略的 `.local-model/`；不安装系统服务、不替换系统库、不自动推理。为 Ubuntu 22.04 配套解包官方 Ubuntu 24.04 更新库，由私有动态加载器使用。

```bash
make local-model-install # 校验所有下载；已安装可复用文件
make local-model-start   # 单独终端前台运行；Ctrl+C 停止
```

另一个终端：

```bash
make local-model-status
make local-model-probe # 仅用固定合成证据检验流式协议、摘要和引用，不读取数据库
make local-model-stop
```

当前固定地址为 `http://127.0.0.1:11435`，模型标签为 `qwen3:4b-q4_K_M`。运行日志位于 `.local-model/server.log`。安装本身不占推理显存；使用时显式启动监督进程，结束后显式停止。若服务因预算不足退出，应先调整游戏/模型占用，检查原业务请求状态，不能自动重发。

学习闭环使用方式：

1. 在 API 环境设置 `LEARNING_LOCAL_ENABLED=true` 并重启 API；Compose 配置写入本机 `.env`，本机 Rust 进程需要导出环境变量。
2. 保存训练结果与结构化证据，在任务内预览分享内容，使用上述地址/模型创建本地核验草稿，分别勾选分享材料和本机计算同意。网页仅保存同意，草稿/批准有效期为 5 分钟。
3. 在宿主机导出该应用的 `DATABASE_URL`（数据库用 loopback 地址），可选导出与 API 同一 Redis 的 `LEARNING_TEXT_REDIS_URL`，然后执行：

```bash
make local-model-start # 保持这个终端运行
# 另一个已导出数据库配置的终端
make local-review OWNER=用户_UUID REQUEST=已批准请求_UUID
cargo run --locked -p api-server --bin local-review -- show 用户_UUID 已批准请求_UUID
```

4. 网页查询原请求、观察私有临时正文或读取已保存建议；逐项核验后再独立确认自评。`local-review` 返回非成功时先查询原请求，未知结果不新建重复调用。完成后 `make local-model-stop`。

`make local-review` 会在领取前核对项目监督进程和显存。直接调用底层 `local-review run` 用于独立部署接入，需由该部署提供资源监督；它本身不执行 NVIDIA 显存检查。模型监督入口不接管其他程序或服务。

## 后续切换 vLLM

补充 [只读 GPU 观测工具](gpu-observation.md)：`make local-gpu-observe DURATION=300 SCENARIO=game-model` 保存最低剩余和采样峰值。工具不会启动模型或游戏，场景标签由用户提供，实际游戏峰值和卡顿仍需对应实测。固定合成 [RSS 质量基准](rss-value-quality.md) 已发现当前 Qwen 4B 的内容不足严格协议失败及注入质量失败；保留人工核验边界，不能将学习材料协议通过推广为所有业务质量通过。

应用依赖供应商无关 `LocalInference`，当前 `llm-local` 使用标准聊天路径、模型别名、JSON 输出和受限 SSE，不绑定 llama.cpp SDK。后续可在同一 loopback 地址提供符合该 profile 的 vLLM 服务，保持页面、授权、数据库和一次派发协议；若地址或模型改变，原授权不能复用，必须重新预览批准。

部署监督与安装脚本当前专用于 llama.cpp，包括 `/props` 睡眠状态及私有加载器。替换时需增加 vLLM 部署入口和资源检查，独立限制上下文/并发/KV cache，并验收 JSON、关闭 thinking、返回模型别名、`stop`/`[DONE]`/EOF、取消和显存峰值。若 vLLM 实际输出不同，由独立适配器实现同一端口，不放宽业务校验。未安装或实测 vLLM，不将通用 HTTP 路径视为已经验收兼容。

## 授权与执行

追加 0040 迁移，只增加本地地址和允许空订阅连接绑定；本地记录在 SQL 的连接 ID/版本为 NULL，私有 JSON 为兼容现有页面输出空字符串/0，加 `local_endpoint` 区分。摘要使用独立 learning-local-consent-v1 域绑定 owner、请求、来源、地址、模型、材料摘要和期限。草稿与订阅共用 5 分钟期限及额度，不保留证据副本；提示预算超限拒绝草稿，不截断材料。

默认 `LEARNING_LOCAL_ENABLED=false`。私有会话/CSRF 下的 `local-model-authorizations`、`approve-local` 分别创建草稿和确认分享/本机计算。订阅批准入口拒绝本地请求；两类领取入口互不领取另一类。来源、材料、自评、技能和计划撤销继续使本地授权/建议失效。

`local-review run OWNER REQUEST ENDPOINT MODEL --use-local` 从数据库重建已批准的原请求，先比对地址/模型，再一次领取。发送前原子保存发送标记，再调用独立适配器；取消、来源改变、超时、崩溃、严格返回失败都不重新领取或发送。Redis 私有临时正文和已保存状态观察沿用现有端口；网页仅保存授权与观察，执行仍须本机明确命令。`local-review show OWNER REQUEST` 查询原状态。建议仍须用户独立核验，不修改自评。

运维审计增加 `execution_kind=local/subscription`，从已授权的连接元数据判断类型，不读取本地地址、模型、摘要或正文，也不扩大最小列权限。

## 分阶段验收记录

2026-10-04：阶段 1 完成本地端口与流式适配器；阶段 2 完成精确授权、一次执行、HTTP 与私有页面，移除初始 Ollama 适配，采用标准聊天 SSE。阶段 2 全仓 Rust、前端 lint/typecheck/build 和 74 项学习双入口浏览器用例通过，桌面/移动截图已检查。

阶段 3 全仓 Rust 检查、4 项显存边界测试和完整隔离 core smoke 通过，包含 162 项仓储、41 项学习 HTTP、10 项学习运维及 6 项 Redis 用例，测试栈已清理。官方模型用固定合成证据通过真实流式、严格 JSON/摘要/引用校验：一次调用约 4.25 秒；100 ms 采样时基线占用 4666 MiB、峰值占用 7702 MiB，模型增量约 3036 MiB，最少剩余 8601 MiB（约 8.4 GiB）。已实测空闲卸载、停止释放与重启；模型实测和数据库生命周期验收分别执行，未验证真实用户完整闭环或实际游戏峰值。没有使用个人订阅凭据、付费 API 或真实用户材料；单次合成材料不能证明能力反馈质量。

完整远程 CI、真实游戏并行压力、用户材料质量、真实订阅、公网/对象存储/向量专项及备份恢复演练不由本批验收替代。默认模型服务保持停止，用户使用时显式启动。

远程 CI 核对发现前两轮索引浏览器回归分别通过 201/200 项后触发 1500 秒整套超时，日志没有用例断言失败；Rust、Web、对象存储和 core 浏览器任务均通过。本批将 Playwright 整套时限改为 35 分钟、外层浏览器命令为 40 分钟、smoke/index CI 任务为 60 分钟，为完整回归与清理留空间；单用例 60 秒、零重试保持。配置语法已检查，新的完整 CI 耗时及结果由本批推送后记录。

参考：[llama.cpp 官方 server](https://github.com/ggml-org/llama.cpp/tree/master/tools/server)、[Qwen 官方 GGUF](https://huggingface.co/Qwen/Qwen3-4B-GGUF)、[vLLM 资源配置](https://docs.vllm.ai/en/latest/configuration/conserving_memory/)。真实调用和质量验收独立记录，夹具不替代真实能力评估。

后续增加[独立候选评估入口](rss-quality-candidates.md)：`MODEL=qwen3-8b` 显式安装/启动官方 8B Q4_K_M，仅用于固定合成 RSS 基准；默认业务 4B 保留。候选上下文 4096，冷启动/唤醒预算 12800 MiB，仍保留 6 GiB 游戏余量及 512 MiB 缓冲。停止当前模型后才能切换；候选模式不执行业务核验/评分，不能复用原业务授权。
