# Local environment baseline

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
