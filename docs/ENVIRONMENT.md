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

## 最近验收记录

以下为历史记录，不代表当前机器已重新检查；命令和范围见 [项目 README](../README.md)。

| 日期 / 环境 | 已通过 | 边界 |
|---|---|---|
| 2026-09-21，macOS / OrbStack | Rust、前端检查及 `make browser-test-index`，16 项 UI 测试 | 索引/检索/引用问答、真实 PostgreSQL/Qdrant、本地模型夹具；一次服务错误重跑未复现，详见检索问答记录 |
| 2026-09-20，macOS / OrbStack | Rust 检查、前端 lint/typecheck/build、完整 `make smoke-objects` | 含事务锁竞争回归、真实 PostgreSQL/MinIO 和重启持久化；未重跑浏览器、Qdrant 专项或真实模型 |
| 2026-09-19，WSL / Docker | 检索/问答检查及 `make smoke-index` | 真实 Qdrant、本地 Embedding/聊天夹具；不验证付费模型质量 |
| 2026-09-16，macOS / OrbStack | `make browser-test-public` | 无头 UI 与公网网页成功导入；PDF 使用 Linux 容器 |
| 2026-09-12，WSL | 核心部署与无头浏览器验收 | 当时使用旧版 Compose；不代表现在仍需该版本 |

macOS 在 2026-09-16 的工具版本为 Rust 1.96.1、Node 22.13.1、npm 10.9.2。此前 WSL 的 Docker/DNS 故障已被后续成功验收取代，不作为当前环境结论。详细记录见 [网页导入](design/sprint-2-web-import.md)、[原文维护](design/sprint-2-original-maintenance.md)和[检索问答](design/sprint-2-retrieval-qa.md)。
