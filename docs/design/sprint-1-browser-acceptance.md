# Sprint 1：WSL 无头浏览器验收

## 方案

用户选择独立 Playwright + Chromium，无需 Windows Chrome、浏览器扩展或应用内 Browser 连接。无头进程在执行测试的宿主机运行（WSL/Linux 或 macOS），不切换桌面、不操作系统鼠标键盘；仍会消耗 CPU/内存。单 worker 串行运行，禁止报告自动弹窗。

测试包独立放在 `tests/browser`，精确锁定 Playwright 版本，不增加 Next.js 生产依赖。生产镜像仍使用应用现有依赖。专属浏览器安装在 Playwright 用户缓存，不读取个人 Chrome profile。

使用完整 Chromium 的无头模式（`channel: "chromium"`），安装命令带 `--no-shell`，无需额外下载 headless-shell。本机曾在可选 shell 下载时遇到代理 TLS 错误；改用已成功安装的完整 Chromium，无需关闭证书校验。

## 命令

```sh
make browser-install
make browser-test
```

切换到新机器后需重新执行 `make browser-install`；WSL 的 Linux 浏览器缓存不能复用于 macOS。Playwright 自动下载宿主平台的 Chromium。macOS 不使用 Linux 的 `--with-deps` 或字体安装命令；PDF 提取依赖随 Linux API 容器安装。

Smoke runner 在隔离 Docker 登录凭据前解析当前 context 的 socket（也遵循 `DOCKER_CONTEXT` 优先于 `DOCKER_HOST` 的规则），支持 OrbStack、Docker Desktop 和 Linux Engine 的本地 Unix socket。远程 daemon 不受支持，因为测试端口必须位于宿主机 loopback；不改变用户当前 context。

隔离配置只保留原配置的插件查找目录和 `cli-plugins` 路径，以便发现 Compose/buildx，不复制登录凭据或凭据助手。启动容器前先检查对应平台的 Chromium 文件是否存在，缺失时提示安装。

显式运行 `make browser-test-public` 可在完整验收中增加公网网页成功导入测试。默认 `make browser-test` 与 CI 跳过该外部网站依赖；显式启用后网络失败作为测试失败报告，不自动跳过。

首次安装需要网络；若 Chromium 报缺少系统库，在有系统安装权限的终端执行 `npm --prefix tests/browser run install-browser -- --with-deps`。CI 自动安装系统依赖。Docker 访问仍需普通终端或经批准的沙箱外执行。

截图需要中文字体，可安装 `fonts-noto-cjk`（CI 已配置）。本机无免密 sudo，改为用户字体目录链接现有 `/mnt/c/Windows/Fonts/msyh.ttc` 并执行 `fc-cache`，无需修改 Windows 或重新分发字体。

`browser-test` 复用 smoke runner：生成随机项目与密码 → 数据库测试 → 生产 Compose/HTTP 验收 → Playwright → finally 清理本次专属数据库、网络和容器。原 `make smoke` 仍只运行 HTTP 验收。浏览器配置拒绝非 loopback 地址，测试管理令牌仅通过运行时环境传递。

## 覆盖

两种配置分别执行同一套测试：Next.js 入口桌面视口 1280×900、Nginx 入口窄屏视口 390×844。

- 使用真实管理员 API 创建专属测试账户；登录由页面表单完成，不注入 Cookie。
- 通过文件 input 选择内存生成的 UTF-8 Markdown，触发实际 File/TextDecoder/FormData 和上传请求。
- 断言导入提示、文件框重置、列表/原文/分块，以及无需手动刷新就更新的概览。
- 验证重复导入不会增加总量，页面重载后会话和文档保留。
- 退出后私人面板卸载；同一浏览器切换用户时空库不包含前一用户文档；再次退出并重载仍未登录。
- 无效 UTF-8、超限文件显示错误；概览 503 后不展示旧计数，解除故障后可重试。仅此 503 场景使用网络拦截，其余使用真实后端。

每例使用独立浏览器上下文和随机账户，不复用个人登录状态；重试为零，失败不会被掩盖。HTML 报告和截图保存在 `tests/browser/playwright-report` / `test-results`，已加入 Git 忽略。禁用 trace、录像和认证状态导出；截图仅包含合成测试数据，报告仍不应公开发布。

## 边界与结果

这是真实 Chromium DOM/交互验收，不是 Windows Chrome 扩展连接测试，也不覆盖操作系统原生文件选择窗口、真实手机、Firefox/WebKit 或完整视觉回归。

2026-09-12 本地执行：`make browser-install` 成功，`make browser-test` 的 4 项 UI 测试、2 项真实 PostgreSQL 测试及全部 HTTP smoke 通过。前端 lint/typecheck 与 `git diff --check` 通过；本轮未改业务代码，生产镜像使用已验证的构建缓存，未重跑完整 Rust 单元测试。已配置 CI，但未推送或验证远程 CI。

修正后连续两轮完整验收通过；补齐字体后的最后一轮 UI 测试耗时 9.0 秒。已人工检查桌面及窄屏概览截图，中文正常显示、计数清晰，窄屏卡片正确换行。每轮临时容器、网络和数据库卷均已清理，浏览器及构建缓存保留。

首轮暴露的测试基础设施问题已修复：API 重启后重新解析 Docker 随机宿主端口；业务错误定位限定在知识库区域，避免 Next.js 隐藏路由播报节点冲突。测试账户创建使用 Node fetch，避免 Playwright 请求诊断记录管理员认证头；临时令牌随隔离环境销毁失效。
