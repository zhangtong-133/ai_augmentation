# 开发环境

## 基线与换机检查

- Rust 1.96+（含 rustfmt、Clippy）、Node.js 20.9+、npm。
- Docker Engine 与 Compose，推荐 Compose v2；`scripts/compose.sh` 兼容旧版 `docker-compose`。
- macOS 和 WSL/Linux 均可开发；依赖、浏览器二进制、Docker 镜像和 `.env` 不随 Git 同步。

```bash
make env-check
make web-install
make compose-config
# 需要 UI 验收时安装当前平台的浏览器
make browser-install
```

首次部署从 `.env.example` 复制 `.env`，替换口令和管理 token。`make compose-config` 只校验示例配置。启动完整栈用 `make stack-up`；只启动数据服务用 `make infra-up`。本机 Rust 进程需显式导出环境变量，不自动读取 `.env`。

## 平台注意事项

- PDF：Linux API 需要 `pdftotext` 和 `prlimit`，分别来自 `poppler-utils`、`util-linux`；Compose 镜像已包含。macOS 使用 Linux API 容器执行提取。
- 浏览器：Playwright 使用独立无头 Chromium，无需启动 Chrome for Testing 或持续前台交互；WSL 安装结果不能复用到 macOS。中文截图需要可用中文字体，CI 安装 `fonts-noto-cjk`。
- Docker：先确认 `docker info` 能访问当前 context。沙箱内 socket 权限错误不等于 daemon 停止，不应因此修改 socket 权限或重装 Docker。
- 网络：网页导入禁用系统代理，API 必须能直连公网 DNS 与 HTTP/HTTPS。模型服务、依赖下载与 Docker 拉取的连通性需分别检查。
- 本机测试：HTTP 适配器测试需要监听回环端口；受限环境需允许本机网络访问。确认服务端口没有冲突后再启动 Compose。
- 本地模型：WSL/Linux 的 llama.cpp 安装与 NVIDIA 显存监督不直接适用于 Mac。macOS ARM64 的[私有问答单次执行](design/ollama-answer-execution.md)使用已安装的 Ollama 和模型，单独预览原生请求、确认并执行；领取前、发送前、运行期间和返回前检查身份与统一内存估计余量，`keep_alive=30`。学习/RSS 尚未接入此适配。[合成问答评估](design/ollama-answer-residency.md)保留独立的 reuse/cold 实验和质量记录。
- 证据角色候选：通过 `CANDIDATE=evidence-role` 显式选择 [v20 合成实验](design/ollama-answer-evidence-role.md)，仅支持 Ollama reuse。先离线预览并冻结全部请求，实际运行继续逐次检查身份与资源；完整报告使用 report v8 / gate v14，不与旧候选或不同运行时混合。该历史完整双轮 132/142，质量门槛失败；实验默认仍为 v15-reuse，实验成绩不控制私有问答工程的启用开关。

## 本地问答单次执行

2026-10-22 起，问答的工程闭环与具体模型的质量验收分别推进。页面已接入精确请求预览、双重确认、单独点击执行和持久化结果恢复；本机 Qwen 的历史质量失败继续保留，不阻塞工程交付。协议和引用校验仍是强制条件，模型答案仍需用户核对，详见[执行设计](design/answer-execution.md)。

执行默认关闭。以下变量仅供与所选模型运行器同机运行的原生 API 进程显式导出，启动 API 本身不会加载模型或执行请求：

| 变量 | 默认 / 要求 |
|---|---|
| `LOCAL_ANSWER_EXECUTION_ENABLED` | 未设置或 `false` 关闭；仅 `true` 启用 |
| `LOCAL_ANSWER_BACKEND` | 默认 `local`（llama.cpp）；macOS ARM64 的原生 Ollama 使用 `ollama` |
| `LOCAL_ANSWER_ENDPOINT` | 启用时必填，固定回环 HTTP 地址；local 示例 `http://127.0.0.1:11435`，Ollama 示例 `http://127.0.0.1:11434` |
| `LOCAL_ANSWER_MODEL` | 启用时必填，必须匹配用户完整预览及当前运行器；Ollama 页面预设为 `qwen3.5:9b` |
| `LOCAL_ANSWER_GUARD_SCRIPT` | 启用时必填，可信资源检查脚本绝对路径；local 使用 `scripts/local-model.mjs`，Ollama 使用 `scripts/ollama-answer-admission.mjs` |
| `LOCAL_ANSWER_NODE_BINARY` | 默认 `node`；显式设置时使用 Node 可执行文件绝对路径 |

`local` 资源检查依赖 Linux/WSL2 的 `/proc`、NVIDIA 和该项目的 `.local-model` 监督状态。部署者先显式启动 `make local-model-start`，再启动配置好的原生 API。领取前和实际发送前各检查一次，每次检查限时 3 秒，模型监督器继续检查显存预算。

`ollama` 使用已有 macOS ARM64 Ollama（至少 0.40.1，且提供身份与 think 能力元数据），运行专用 `answer-admit` / `answer-monitor` 检查；API 首次成功准入固定模型和运行器身份，变化后拒绝新执行或停止等待，直到部署者显式重启 API。领取前、发送前、运行期间每 2 秒和返回结果前检查，单次检查上限 10 秒、脚本总截止 7 秒。准入统一内存估计至少保留 6 GiB 加 512 MiB；冷加载还计入 1.5 倍模型大小，复用同一模型/运行器/上下文时额外计入 1 GiB 推理缓冲。在途检查只要求基础余量，不重复扣除加载预算。授权保存模型别名和完整请求，权重 SHA 的固定仅属于 API 进程内准入。详见[Ollama 执行设计与导出示例](design/ollama-answer-execution.md)。

检查子进程只接收检查模式、固定 endpoint 和 model，不接收问题、材料或会话。API 进程一次仅允许一个执行；取消、超时或资源检查失败会停止等待，不停止或卸载共享 Ollama，也不能保证其立即停止计算。该保护不提供共享服务的外部排他。原授权不会恢复或自动重发。

标准 Compose API 镜像没有 Node、宿主模型及其监督环境，因此不透传这些变量。Ollama 需在同机运行 native API；Web 和数据服务仍可单独使用 Compose。固定目标、后端或完整请求变化后必须重新预览和确认；Ollama 使用独立 `ollama-local-answer-v1`，不能转换或复用旧 `local-knowledge-answer-v2` 授权。工程验收使用确定性 HTTP/SSE/NDJSON 夹具，不以真实 Qwen 质量成绩作为启用前置。

迁移 `0043_answer_executions.sql` 追加执行与审计表；`0044_answer_authorization_backend.sql` 保存后端选择，旧记录默认 `local`，保留原请求字节和摘要。详情读取会重新核对权威来源；来源改变、取消或失败后清除问题和结果正文。列表仅含元数据。结果未知时只查询原请求，系统不会扫描旧记录自动恢复执行。

## CI 故障排查

GitHub Actions 使用 `stable` Rust，可能比本机工具链更新。排查 Clippy 错误时先比较日志中的 `rustc`/Clippy 版本；本机较旧版本通过不能证明当前 CI 通过。2026-10-09 的 [失败构建](https://github.com/zhangtong-133/ai_augmentation/actions/runs/37893764769) 使用 Rust 1.99，新的 `assert_is_empty` 检查拒绝了 Ollama 测试中的布尔空集合断言。当前测试改为比较带元素类型的空数组，保留原断言并输出失败时的实际引用列表。

2026-10-12 检查[上一轮 v13 构建](https://github.com/zhangtong-133/ai_augmentation/actions/runs/38042115645)，其余 smoke、web、index、objects 作业均成功；同一 Rust 1.99 规则再次拒绝新增的 `answer.citations.is_empty()` 断言。v14 测试统一使用带元素类型的空数组比较，继续保留 `-D warnings`，本机默认 1.96 不变。复核时另装指定 Rust 1.99.0 / Clippy 0.1.99，独立构建目录完整 fmt、Clippy `-D warnings` 与 326 项测试通过，直接验证 CI 差异；默认仍为 1.96。

继续驻留复用开发前核对 [v14 的 CI 38199354985](https://github.com/zhangtong-133/ai_augmentation/actions/runs/38199354985)，提交 `721989f` 的 Rust、Web、smoke、index、objects 全部成功。

同一次构建的 smoke 在 Playwright 安装系统依赖时，卡住于 `http://azure.archive.ubuntu.com` 的 APT 索引下载，项目 smoke 尚未启动。smoke/index 现共用 [浏览器安装 action](../.github/actions/setup-browser/action.yml)：将 runner 的 APT 镜像列表替换为 Ubuntu 官方 HTTPS archive，设置每次连接 30 秒超时、最多 3 次重试，索引更新错误直接失败；Chromium 安装步骤最多 10 分钟，字体安装最多 5 分钟，原有作业 60 分钟限时保持不变。这些系统配置仅写入临时 GitHub runner，本机 `make browser-install` 的行为保持不变。

本次修复在 macOS / Rust 1.96.1 下通过 `make check`（322 项通过、286 项需真实服务的测试忽略）、前端 lint/typecheck/build；工作流与 composite action 的 YAML/脚本语法另行检查。推送后的 [CI 37903618314](https://github.com/zhangtong-133/ai_augmentation/actions/runs/37903618314) 五个作业全部通过，确认 Ubuntu runner 的 Rust 1.99 检查和 APT 安装成功；smoke/index 浏览器安装分别约 26/31 秒。

## 最近验收记录

以下为历史记录，不代表当前机器已重新检查；命令和范围见 [项目 README](../README.md)。

| 日期 / 环境 | 已通过 | 边界 |
|---|---|---|
| 2026-10-22，macOS ARM64 / OrbStack，问答执行闭环夹具 | Rust 1.96.1 fmt/Clippy 与 367 项工作区测试、最终 Clippy 复核；前端 lint/typecheck/build 及 16 项单测、6 项显存预算 Node；300 项真实服务集成（含 186 项 PostgreSQL、6 项授权/执行 HTTP）、生产镜像/index smoke 与 16 项桌面/窄屏 UI；截图、隔离资源清理已核对 | 预览和确认不发送，单次领取、取消竞态、晚到丢弃、引用校验、持久化恢复及默认关闭均通过。未调用真实模型，未跑其他页面全量 UI、MinIO/公网导入、Linux/NVIDIA 实机、游戏/vLLM；当前 guard 需 native Linux/WSL2 同机监督环境，macOS Ollama 尚需单独适配，见[执行记录](design/answer-execution.md) |
| 2026-10-22，macOS ARM64 / OrbStack，Ollama 0.40.7 / 逐项诊断 v2 | Rust 1.96.1 完整检查、58 项问答及显存 Node、前端 lint/typecheck/build 和 11 项单元测试；281 项 core smoke、生产镜像 v14–v19 各 71 道冻结清单；v17/v19 三题各一次，17 份历史 v1 与六份 v2 诊断严格重读，三份历史 gate 逐字段保持 | 新诊断定位两题多加未问属性、一题一致选择伪 SYSTEM 命令；v17 一题正确弃权仍依赖证据分歧。不同运行时观察不合并，不代表质量修复；默认及私有执行状态保持。两测试栈资源、模型自动释放、锁与私有权限已核对；未重跑完整模型双轮、浏览器/index/MinIO/公网、游戏/vLLM，见[诊断记录](design/ollama-answer-requirement-diagnostics.md) |
| 2026-10-15，macOS ARM64 / OrbStack，Ollama 0.40.3 / v19-evidence-bound 约束实验 | Rust 1.96/1.99 各完整检查及 351 项测试、43 项问答及 10 项显存预算/观测 Node、前端 lint/typecheck/build 及 11 项单元测试；281 项 core smoke、生产镜像 v14–v19 各 71 道冻结清单；完整双轮 142 题协议及原文范围校验完成、六题单次诊断 | 质量 132/142，低于 v17 的 136/142；正式比较保留两次整题改善、六次退步，gate v13 失败。沉默题通过仍依赖证据分歧；默认及私有执行状态保持。报告权限、模型自动释放、锁及测试栈清空已核对；未跑本机浏览器/index/MinIO/公网、游戏/vLLM，见[实验记录](design/ollama-answer-evidence-bound.md) |
| 2026-10-15，macOS ARM64 / OrbStack，Ollama 0.40.3 / v18-question-first 顺序实验 | Rust 1.96/1.99 各完整检查及 348 项测试、42 项问答及 10 项显存预算/观测 Node、前端 lint/typecheck/build 及 11 项单元测试；281 项 core smoke、生产镜像 v14–v18 各 71 道冻结清单 | 前七套双轮 114/118；支持首轮第八题 verification_decision 协议拒绝，按冻结规则停止；125 题协议完成、1 题拒绝、16 题未发，不能给出完整总分。十四份完整/一份部分报告保留，gate/compare 拒收；未运行预定十一题诊断，默认及私有执行状态保持。模型释放、锁及测试栈清空已核对；未跑浏览器/index/MinIO/公网、游戏/vLLM，见[实验记录](design/ollama-answer-question-first.md) |
| 2026-10-15，macOS ARM64 / Ollama 离线比较器 | 完整 Rust fmt/Clippy 及 345 项测试、51 项问答/显存 Node、前端 lint/typecheck/build 及 11 项单元测试；正式 compare 复核两组历史双轮，v15/v16/v17 原 gate 逐字段一致 | v15 → v17 六次整题改善但四项检查退步；v16 → v17 二十次改善、六次退步；两份有界私有报告权限核对通过。未调用模型、未重跑 smoke/PostgreSQL/浏览器/index/MinIO/公网、游戏/vLLM；默认及私有执行状态保持，见[比较记录](design/ollama-answer-comparison.md) |
| 2026-10-15，macOS ARM64 / OrbStack，Ollama 0.40.3 / v17-guarded 三步实验 | Rust 1.96/1.99 各完整检查及 345 项测试、32 项问答及 10 项显存预算/观测 Node、前端 lint/typecheck/build 及 11 项单元测试；281 项 core smoke、生产镜像 v14–v17 各 71 道冻结清单及双入口 HTTP | 完整双轮 136/142，相对 v15 三题各两轮改善、无新增失败题；既有 injection 失败方式变化及四项布尔检查回归已记录。十一题单次诊断区分数量失配、证据分歧及一致误选；质量门槛仍失败，默认 v15 与私有执行关闭状态保持；模型释放、锁解除、测试栈清空已核对；未跑浏览器/index/MinIO/公网、游戏并行/vLLM，详见[实验记录](design/ollama-answer-guarded.md) |
| 2026-10-15，macOS ARM64 / OrbStack，Ollama 0.40.3 / v16-question 实验 | Rust 1.96/1.99 各完整检查及 340 项测试、34 项问答/显存 Node、前端 lint/typecheck/build 及 11 项单元测试；281 项 core smoke、生产镜像 v14/v15/v16 各 71 道冻结清单及双入口 HTTP | 完整双轮 122/142，低于同运行时 v15 的 130/142；五题各两轮改善、九题各两轮回归，质量门槛退出 2；八题单次诊断复现需求拆分失配及类型正确但支持错误，默认 v15 与私有执行关闭状态保持；模型释放、锁解除、测试栈清空已核对；未跑浏览器/index/MinIO/公网、游戏并行/vLLM，详见[实验记录](design/ollama-answer-question.md) |
| 2026-10-14，macOS ARM64 / OrbStack，Ollama 0.40.3 / v15 诊断 | Rust 1.96/1.99 各完整检查及 332 项测试、30 项问答/显存 Node、前端 lint/typecheck/build 及 11 项单元测试；281 项 core smoke、生产镜像冷热冻结清单及双入口 HTTP | 九题合成诊断复现六道历史失败、三道对照通过，区分证据分歧和类型判断；原十六份报告重算仍为 130/142，诊断不计入门槛、私有执行关闭；空闲释放、项目锁及测试栈清理已核对；未重跑完整模型双轮、浏览器/index/MinIO/公网、游戏并行/vLLM，详见[诊断记录](design/ollama-answer-diagnostics.md) |
| 2026-10-12，macOS ARM64 / OrbStack，Ollama 0.40.3 / v15-reuse | Rust 1.96/1.99 各完整检查及 329 项测试、27 项问答/显存 Node、前端检查/构建及 11 项单元测试；281 项 core smoke、生产镜像冷热模式各 71 道冻结清单及双请求预览核对 | 完整双轮 130/142，568 个检查与 v14 一致；284 次许可为 1 次 cold、283 次 reuse，空闲约 30 秒自动释放，中位耗时 11.855→7.496 秒；新 v9/旧 v8 门槛独立重算一致，质量仍失败、私有执行关闭；模型释放、锁解除及测试栈清空；未跑浏览器/index/MinIO/公网、游戏并行/vLLM，详见[记录](design/ollama-answer-residency.md) |
| 2026-10-12，macOS ARM64 / OrbStack，Ollama 0.40.3 / v14 | 326 项 Rust 测试与完整检查、17 项问答 Node、前端检查/构建及 11 项单元测试；281 项 core smoke、生产镜像八套 71 道双请求清单逐字段核对 | 先重跑未修改 v13 完整 116/142，v14 完整双轮 130/142，九题各两轮改善、两题各两轮回归；门槛退出 2、私有执行关闭；每题两次资源保护，卸载、锁释放、测试栈清空；未跑浏览器/index/MinIO/公网、游戏并行/vLLM |
| 2026-10-10，macOS ARM64 / OrbStack，Ollama 0.40.2 / v13 | 331 项 Rust 测试与完整检查、12 项问答 Node、前端检查/构建及 11 项单元测试；281 项 core smoke（含 175 项 PostgreSQL）、生产镜像八套 71 道冻结请求逐字段核对 | 完整双轮 116/142，实际值/可用性各轮 12/12、支持对照各轮 10/12；原七套从 v12 的 102/118 降至 96/118，整体质量未改善，门槛退出 2、私有执行关闭；模型卸载、锁释放、测试栈清空；未跑浏览器/index/MinIO/公网、游戏并行/vLLM |
| 2026-10-10，macOS ARM64 / OrbStack，Ollama 0.40.2 / v12 | 330 项 Rust 测试与完整检查、12 项问答 Node、前端检查/构建及 11 项单元测试；281 项 core smoke（含 175 项 PostgreSQL）、生产镜像八套 71 道冻结请求逐字段核对 | 完整双轮 120/142，旧实际值/可用性各轮 12/12；原七套从 v11 的 114/118 降至 102/118，新支持套件各轮 9/12，门槛退出 2、私有执行关闭；模型卸载、锁释放、测试栈清空；未跑浏览器/index/MinIO/公网、游戏并行/vLLM |
| 2026-10-10，macOS ARM64 / OrbStack，Ollama 0.40.2 / v11 | 327 项 Rust 测试与完整检查、12 项问答 Node、前端检查/构建及 11 项单元测试；281 项 core smoke（含 175 项 PostgreSQL）、生产镜像七套 59 道冻结请求逐字段核对 | 最终完整双轮 114/118，原六套 94/94，实际值/可用性各轮 10/12；两题各两轮回归，门槛退出 2、私有执行关闭；一次 Redis 并发失败未复现，独立五次并发/三次全套及最终 smoke 通过，详见[记录](design/ollama-answer-extraction.md#最终严格字符串解析器复评)；测试栈清空，未跑浏览器/index/MinIO/公网、游戏并行/vLLM |
| 2026-10-10，macOS ARM64 / OrbStack，Ollama 0.40.2 / v10 | 326 项 Rust 测试与完整检查、12 项问答 Node、前端检查/构建及 11 项单元测试；281 项 core smoke（含 175 项 PostgreSQL）、生产镜像七套 59 道冻结请求逐字段核对 | 完整双轮 108/118，新可用性每轮 11/12；原六套 86/94 低于同运行时 v9 的 92/94，旧缺负责人误答持续且新增误弃权回归；门槛退出 2、私有执行未接入；测试栈清空，未跑浏览器/index/MinIO/公网、游戏并行/vLLM |
| 2026-10-10，macOS ARM64 / OrbStack，Ollama 0.40.2 / v9 | 324 项 Rust 测试与完整检查、12 项问答 Node、前端检查/构建及 11 项单元测试；281 项 core smoke（含 175 项 PostgreSQL）、生产镜像六套 47 道清单逐字段核对 | 同运行时 v8 仍 70/74；v9 原五套双轮 74/74，新混合资料双轮 9/10，共 92/94；缺负责人且旁有诱导仍误答，门槛退出 2、私有执行未接入；测试栈已清空，未跑浏览器/index/MinIO/公网、游戏并行/vLLM |
| 2026-10-09，macOS ARM64 / OrbStack，Ollama v8 | 322 项 Rust 测试与完整检查、12 项问答 Node、前端检查/构建及 11 项单元测试；284 项真实服务/index smoke、4 项授权 UI、生产镜像五套离线预览 | 74 次协议/生产引用完成、70 次质量通过；基准/覆盖/新决策双轮 7/7、10/10、4/4，两道诱导材料题各两轮误弃权；门槛退出 2，私有执行关闭；测试栈已清空，未跑全量 UI/MinIO/公网、游戏并行/vLLM |
| 2026-10-09，macOS ARM64，Ollama v7 | 321 项 Rust 测试与完整检查、12 项问答 Node，前端检查/构建及 11 项单元测试；66 次原生协议/生产引用完成，挑战双轮 8/8 | 基准双轮 6/7、覆盖 6/10、原文对照 4/8，48/66 质量通过，门槛退出 2，私有执行关闭；未跑 Docker/服务/UI/MinIO/公网、游戏压力/vLLM |
| 2026-10-09，macOS ARM64 | 321 项 Rust 测试与完整检查、12 项问答 Node、前端检查/构建及 11 项单元测试；33 个 v6 离线请求预先冻结 | 原文选择候选首轮前四题通过、注入题分类/选择矛盾协议拒绝，2 题未发送；完整质量门槛未完成，私有执行关闭；该快照未跑 Docker/服务/UI/MinIO/公网、游戏并行/vLLM |
| 2026-10-09，macOS ARM64 / OrbStack | 317 项 Rust 测试与完整检查、前端检查/构建、12 项问答 Node 测试、284 项真实服务/index smoke、4 项授权 UI；修复 RSS 测试夹具并行争用两个名额 | Ollama v5 原 `partial_answer` 双轮通过，但基准双轮首题协议失败、挑战双轮 6/8、新覆盖双轮 9/10；质量门槛未通过，私有执行保持关闭；未重跑全量 UI/MinIO/公网，未验游戏并行/vLLM |
| 2026-10-08，macOS ARM64 / OrbStack | Rust 1.96.1 完整检查（308 项测试）、前端检查/构建、284 项真实服务集成与 index smoke、4 项授权 UI；已有 Ollama 0.40.1 / Qwen3.5-9B 的真实双轮评估 | 基准两轮 7/7、独立挑战两轮 7/8，质量门槛失败，私有执行仍关闭；未重跑全量 UI、MinIO、公网导入，未验游戏并行/vLLM |
| 2026-09-21，macOS / OrbStack | Rust、前端检查及 `make browser-test-index`，16 项 UI 测试 | 索引/检索/引用问答、真实 PostgreSQL/Qdrant、本地模型夹具；一次服务错误重跑未复现，详见检索问答记录 |
| 2026-09-20，macOS / OrbStack | Rust 检查、前端 lint/typecheck/build、完整 `make smoke-objects` | 含事务锁竞争回归、真实 PostgreSQL/MinIO 和重启持久化；未重跑浏览器、Qdrant 专项或真实模型 |
| 2026-09-19，WSL / Docker | 检索/问答检查及 `make smoke-index` | 真实 Qdrant、本地 Embedding/聊天夹具；不验证付费模型质量 |
| 2026-09-16，macOS / OrbStack | `make browser-test-public` | 无头 UI 与公网网页成功导入；PDF 使用 Linux 容器 |
| 2026-09-12，WSL | 核心部署与无头浏览器验收 | 当时使用旧版 Compose；不代表现在仍需该版本 |

macOS 在 2026-09-16 的工具版本为 Rust 1.96.1、Node 22.13.1、npm 10.9.2。此前 WSL 的 Docker/DNS 故障已被后续成功验收取代，不作为当前环境结论。详细记录见 [网页导入](design/sprint-2-web-import.md)、[原文维护](design/sprint-2-original-maintenance.md)和[检索问答](design/sprint-2-retrieval-qa.md)。
