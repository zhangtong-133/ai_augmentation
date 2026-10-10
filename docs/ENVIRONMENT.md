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
- 本地模型：WSL/Linux 的 llama.cpp 安装与 NVIDIA 显存监督不直接适用于 Mac。macOS ARM64 可显式使用[已有 Ollama 的合成问答评估](design/ollama-answer-extraction.md)，使用统一内存估算保护，每题结束卸载模型；尚未接入私有问答或学习/RSS 执行。

## CI 故障排查

GitHub Actions 使用 `stable` Rust，可能比本机工具链更新。排查 Clippy 错误时先比较日志中的 `rustc`/Clippy 版本；本机较旧版本通过不能证明当前 CI 通过。2026-10-09 的 [失败构建](https://github.com/zhangtong-133/ai_augmentation/actions/runs/37893764769) 使用 Rust 1.99，新的 `assert_is_empty` 检查拒绝了 Ollama 测试中的布尔空集合断言。当前测试改为比较带元素类型的空数组，保留原断言并输出失败时的实际引用列表。

同一次构建的 smoke 在 Playwright 安装系统依赖时，卡住于 `http://azure.archive.ubuntu.com` 的 APT 索引下载，项目 smoke 尚未启动。smoke/index 现共用 [浏览器安装 action](../.github/actions/setup-browser/action.yml)：将 runner 的 APT 镜像列表替换为 Ubuntu 官方 HTTPS archive，设置每次连接 30 秒超时、最多 3 次重试，索引更新错误直接失败；Chromium 安装步骤最多 10 分钟，字体安装最多 5 分钟，原有作业 60 分钟限时保持不变。这些系统配置仅写入临时 GitHub runner，本机 `make browser-install` 的行为保持不变。

本次修复在 macOS / Rust 1.96.1 下通过 `make check`（322 项通过、286 项需真实服务的测试忽略）、前端 lint/typecheck/build；工作流与 composite action 的 YAML/脚本语法另行检查。推送后的 [CI 37903618314](https://github.com/zhangtong-133/ai_augmentation/actions/runs/37903618314) 五个作业全部通过，确认 Ubuntu runner 的 Rust 1.99 检查和 APT 安装成功；smoke/index 浏览器安装分别约 26/31 秒。

## 最近验收记录

以下为历史记录，不代表当前机器已重新检查；命令和范围见 [项目 README](../README.md)。

| 日期 / 环境 | 已通过 | 边界 |
|---|---|---|
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
