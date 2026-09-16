# Local environment baseline

## 2026-09-16 macOS 开发环境

当前宿主为 Apple Silicon macOS，Rust/Cargo 1.96.1、Node 22.13.1、npm 10.9.2。OrbStack Docker Engine 29.4.0 与 Compose v2 可用；沙箱内不可访问 socket 不代表 daemon 停止。WSL 的浏览器及 Docker 镜像缓存不会随 Git 提交迁移到 Mac。

已运行 `make browser-install` 安装 Playwright 1.63.0 与 mac-arm64 Chromium 153.0.8010.12，并独立启动无头浏览器验证中文 DOM 读取。PDF 提取继续通过 Linux API 容器中的 `prlimit` 和 `pdftotext` 执行，macOS 宿主未安装这些运行依赖。

已通过 `make check`（23 个常规测试）、前端 lint/typecheck/build、Compose 配置校验，以及此前因 WSL DNS 失败的 `live_public_html_import` 公网测试。`make browser-test-public` 的两项真实 PostgreSQL 测试、完整 HTTP smoke 和 10 项浏览器测试全部通过（UI 阶段 38.4 秒）。完整记录见 [网页导入设计](design/sprint-2-web-import.md)。

验收脚本现按当前 Docker context/环境变量解析本机 socket，不再写死 Linux 路径；匿名 Docker 配置保留插件发现路径，避免隐藏 OrbStack 的 buildx。新增 `make browser-test-public`，在原有回归上显式启用公网成功导入验收。

## 2026-09-12 WSL 浏览器验收

已安装独立 Playwright 1.63.0 与 Chromium 153.0.8010.12，`make browser-install` 成功；`make browser-test` 的 4 项桌面/窄屏交互测试、2 项 PostgreSQL 测试和 HTTP smoke 全部通过。无需连接 Windows Chrome 或使用前台鼠标键盘；CI 已接入，远程运行尚未验证。详见 [浏览器验收设计](design/sprint-1-browser-acceptance.md)。

本机没有可用中文字体，且 sudo 需要密码；已将现有 Windows `msyh.ttc` 链接到 `/home/zt/.local/share/fonts/personal-ai-msyh.ttc` 并刷新字体缓存，未复制或修改 Windows 字体。CI 使用 `fonts-noto-cjk`。Docker legacy builder 缺少 buildx 的警告仍存在，但不阻塞此次测试。

## 2026-09-12 核心部署验收

已在获准的沙箱外运行 `make smoke`，真实 PostgreSQL 两个集成测试均通过；生产 API/Web Docker 镜像、Nginx 与 Next.js 双入口、登录/导入/用户隔离/概览/重启持久化/退出流程通过。参见 [验收记录](design/sprint-1-acceptance.md)。这取代下方“数据库测试尚未执行”的历史状态，不代表浏览器交互或后续向量/对象存储已验收。

本机仍使用 Compose 1.29.2 和 Docker legacy builder。测试发现旧 Desktop 凭据助手不可用，验收脚本改用独立临时匿名 Docker 配置；构建显式接收宿主标准代理变量，避免依赖下载超时。无需修改用户凭据或 socket 权限。后端 Dockerfile 已避免开发 stable 覆盖触发额外工具链下载。

## 2026-09-07 复查（替代下方历史 Docker 结论）

经授权在沙箱外检查，Docker Engine 27.5.1 可连接，`docker ps` 成功；当前用户有效组包含 docker，宿主 WSL 的 PID 1 为 systemd。沙箱内 socket 访问返回 operation not permitted，不能据此判断本地服务停止。后续 Docker 操作需在普通终端或获准的沙箱外环境执行，无需修改 socket 权限或重装引擎。

`./scripts/compose.sh --env-file .env.example ps` 成功连接引擎，项目当前没有容器。Compose v2 插件是指向 `/mnt/wsl/docker-desktop/cli-tools/` 的失效历史链接；项目包装脚本已回退至可用的 docker-compose 1.29.2。v2 尚未安装，升级仍为独立待办；未删除旧链接或修改系统包。

环境检查脚本已将“不可达即服务未启动”的提示改为进程访问受限提示。以下为 2026-09-06 历史记录，不代表当前引擎状态。

验证：沙箱外 `make env-check` 报告 daemon reachable，`make compose-config` 通过；`docker run --rm hello-world` 成功拉取镜像并启动容器。测试容器退出后自动清理，镜像保留在本地缓存。该测试不代表项目数据库集成测试或完整应用 smoke test 已执行。

采集日期：2026-09-06（Asia/Shanghai）

本轮补充验证：在沙箱外执行 docker info / docker ps，同样返回 Cannot connect to the Docker daemon。确认当前 Docker 不可用，但仅凭这个结果不能判断宿主机是否安装或启用了 Docker Desktop。PostgreSQL 实例尚未就绪，数据库集成测试尚未在本机执行。

| 项目 | 检查结果 | 结论 |
|---|---:|---|
| OS | WSL2 / Linux x86_64 | 可用 |
| Rust | rustc/cargo 1.96.1 stable | 可用 |
| Node.js | 20.15.1 | 可用 |
| npm | 10.7.0 | 可用 |
| Corepack | 0.28.1 | 可用 |
| pnpm | 未安装 | 非阻塞，本工程先使用 npm |
| Docker CLI | 27.5.1 | 可用 |
| Docker Compose v2 | 未安装 | 建议补齐 |
| docker-compose v1 | 1.29.2 | 临时兼容 |
| Docker daemon | 当前 WSL 会话中 socket 不可用 | 启动 Compose 前需修复 |
| protoc | 3.12.4 | 可用，MCP/gRPC 后续可能需升级 |
| just | 未安装 | 非阻塞，本工程使用 Make |
| 磁盘 | 约 825 GiB 可用 | 充足 |
| 内存 | 约 15 GiB，总可用约 13 GiB | 足够本地基础栈 |

注意：受执行沙箱限制，端口监听状态未能通过 netlink 检查。启动完整栈前应再次检查 3000、8080、5432、6333、6379、9000、9001 端口。

Docker 诊断：当前 WSL 没有启用 systemd Docker 服务，也看不到 `/var/run/docker.sock`。`/etc/group` 的 `docker` 组包含用户 `zt`，但当前登录会话的有效组列表尚未包含该组。建议先确认 Docker Desktop 正在运行并为本 WSL distribution 开启 integration，然后重启 WSL/终端会话再执行 `docker info`。

前端固定使用 Next.js 16.3.4 / React 19.2.8 / TypeScript 5.9.3。Next.js 本身要求 Node.js 20.9+，当前版本满足；`typescript-eslint` 暂时固定为 8.46.0，以避免其较新的传递依赖要求 Node.js 20.19+。

## PDF 导入运行依赖

Linux API 进程需要 `poppler-utils`（pdftotext）和 `util-linux`（prlimit）；Compose 运行镜像自动安装。本机缺少 Poppler 时使用 Compose 验收 PDF；普通 Rust 检查和 Markdown 导入不依赖该可执行文件。

## 网页抓取依赖

网页导入使用 Rust reqwest/rustls 和 scraper，不新增系统包。API 需直接访问公网 DNS 与 HTTP/HTTPS；抓取禁用系统代理。适配器 HTTP 单元测试需允许监听本机回环端口，受限沙箱内运行 `make check` 时需要授予该权限。

2026-09-15：本机直接 DNS 查询无法解析 `example.com`（独立 curl 返回 `Could not resolve host`）；真实公网网页导入补充验收未通过。需恢复公网 DNS/直连网络后运行 `cargo test -p personal-ai-web-import live_public_html_import -- --ignored`，再执行浏览器成功导入验收。
