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

## 最近验收记录

以下为历史记录，不代表当前机器已重新检查；命令和范围见 [项目 README](../README.md)。

| 日期 / 环境 | 已通过 | 边界 |
|---|---|---|
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
