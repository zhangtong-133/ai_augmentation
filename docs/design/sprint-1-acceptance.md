# Sprint 1：真实数据库与部署验收

## 目标与范围

`make smoke` 使用真实 PostgreSQL 和生产 Dockerfile 构建的 API、Next.js，加上仓库 Nginx 配置，验证当前 Sprint 1/Markdown 功能。使用 Node.js 20.9+ 内置 HTTP/断言与进程 API，无新增 npm 依赖。

独立 `compose.smoke.yaml` 只包含 postgres、api-server、web、nginx；不启动尚未使用的 Redis、Qdrant、MinIO、worker 或 scheduler，因此这是核心部署链路验收，不是全部规划服务的功能验收。测试复用原始数据库初始化脚本，随后运行 SQLx 迁移，验证两者兼容。

后端生产 Dockerfile 使用版本化 Rust 基础镜像自带工具链，不再复制开发机的 `stable` 覆盖文件，避免构建时额外下载和切换工具链。宿主的 rustfmt/Clippy 检查仍使用仓库 `rust-toolchain.toml`。

## 运行与隔离

```sh
make smoke
```

需要本地 Unix socket Docker、Compose v2 或兼容 v1、Rust 工具链和 Node。首次执行需联网拉取公开镜像及构建依赖。沙箱禁止访问 Docker 时，在普通终端或获准的沙箱外运行；脚本不自动提权。

构建按需转发宿主 `HTTP_PROXY`、`HTTPS_PROXY`、`NO_PROXY` 标准变量；代理须能从 Docker 构建网络访问。它们使用 Docker 的预定义代理 build args，不写入应用运行环境，不在脚本日志中输出。

每次生成随机 Compose 项目名、管理令牌及数据库密码，所有映射端口仅绑定 `127.0.0.1` 并由 Docker 分配。显式使用无凭据的 `infra/smoke.env`，不读取开发 `.env`。临时 Docker 客户端配置仅用于匿名拉取公开镜像，避免旧 Desktop 凭据助手干扰；不会修改已有 Docker 登录信息。本脚本明确连接本机 `/var/run/docker.sock`，不支持远程 context。

数据库存储使用本次项目专属命名卷。成功或失败均在 finally 中执行仅针对该项目的 `down --volumes --remove-orphans`；不执行全局 prune，不删除普通项目数据。镜像与构建缓存保留。强制终止进程/断电无法保证清理，按输出项目名定位残留资源后处理，不能使用广泛清理命令。

## 验收断言

1. 启动 PostgreSQL，在动态端口执行 `make test-postgres`，显式运行两个原本 ignored 的持久化测试。
2. 构建并启动生产 API/Web 镜像和 Nginx，等待真实 readyz，检查首页 HTTP 响应及 healthz。
3. 通过 API 管理接口创建两名测试用户、设置密码；分别从 Next.js 代理与 Nginx 登录，检查 Cookie 属性。
4. 验证无会话 401、缺少 CSRF 头 403、空库、Markdown 导入、重复内容 409、其他用户访问 404。
5. 两个入口均校验文档列表/原文、no-store、总文档/文本块/今日导入统计；UTC 边界按服务端返回的范围计算。
6. 重启数据库和 API，确认原文及会话保留；退出后两个入口拒绝旧 Cookie，重新登录仍可读取文档。

每个 HTTP 请求有超时，启动轮询有上限，外部命令有 20 分钟上限。错误不打印 Cookie、令牌、密码或数据库连接串。CI 新增独立 smoke job；本地成功不等同于远端 CI 已通过。

## 验证边界

该脚本执行真实 HTTP 流程，不驱动浏览器点击，不能替代 React 交互、文件选择器或视觉验收。完整生产环境的 TLS/Secure Cookie、向量检索、对象存储、后台任务不在本轮范围内。

## 执行结果

2026-09-12 本地验证：

- `make check` 通过，14 个常规 Rust 测试通过；普通测试命令仍按约定跳过两个数据库测试。
- `make smoke` 在独立 PostgreSQL 中显式执行两个数据库测试，2 个通过、0 个跳过；生产 API/Web 镜像构建成功，双入口及重启/退出断言全部通过。
- 修复后连续两次完整运行成功，第二轮命中镜像构建缓存，重新创建独立数据库并完成全部断言及清理。
- 前端 lint、typecheck、宿主生产 build 与容器生产 build 均通过；原 Compose 配置校验通过。
- 测试项目的容器、网络、数据卷已自动清理，镜像/构建缓存保留；没有删除个人数据库。
- CI smoke job 已配置，本轮尚未提交推送，未宣称远端执行通过。浏览器点击/视觉验收未执行，路线图单独保留待办。
