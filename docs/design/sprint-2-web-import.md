# Sprint 2：网页 URL 导入

后续更新：原文对象存储现已实现，启用方式、兼容规则与跨存储事务边界见 [MinIO / S3 原文存储](sprint-2-object-storage.md)。以下保留本阶段交付时的设计。

## 用户行为与接口

知识库新增「导入网页」表单，接受公开 HTTP/HTTPS 网页地址、可选标题和标签。成功后自动刷新列表与今日概览；详情显示「网页提取文本」和最终来源链接。原始 HTML 仅保存，不在页面执行或通过详情 JSON 返回。正文保持普通文本语义，包括字面的 Markdown 符号。

`POST /api/documents` 沿用 Cookie 会话与 `X-Requested-With: personal-ai`。`markdown`、`pdf_base64`、`url` 必须恰好提供一项；URL 导入不允许自行提供 `source`，以最终抓取地址为准。示例：

```json
{"url":"https://example.com/article","title":"","tags":["学习"]}
```

标题未填时使用 HTML title（归一化空白、最多 200 字符），缺失时使用来源地址的前 200 字符。列表和详情的 `source_type` 增加 `web_page`，兼容已有详情 `markdown` 字段，该字段装载提取正文。

去重键为 `web:` 加 UTF-8 HTML 原文 SHA-256；同一用户的相同 HTML 不重复导入（409），不同用户可分别导入。同一 URL 内容更新后可保存新快照，不进行 URL 唯一去重。

## 抓取边界

- 只允许 HTTP 80、HTTPS 443；拒绝 URL 登录信息、控制字符、超长 URL。
- 拒绝 loopback、私网、链路本地、组播、文档地址、IPv4 特殊地址及 IPv6 过渡/特殊地址；DNS 返回的所有地址必须通过检查。IPv6 仅开放普通全球单播段。
- 每一跳单独解析并检查 DNS，使用新建客户端将连接固定到本次检查结果，防止校验后再次解析得到内网地址。
- 禁用系统代理、自动重定向、Referer 和压缩；不转发 Cookie、Authorization 或管理凭证。TLS 使用正常证书校验。
- 手动处理 301/302/303/307/308，最多跟随 3 次；每次重新验证目标，禁止 HTTPS 降级 HTTP。
- 连接限时 3 秒，DNS、所有重定向、读取和解析整体等待限时 8 秒；每个 API 实例最多同时两个导入任务。
- 仅接收 `text/html`，支持 UTF-8（或 ASCII）声明/未声明但可严格按 UTF-8 解码的正文；HTML 最大 1 MiB，按流累计检查，不仅信任 Content-Length。请求 identity 编码，拒绝服务器仍返回的压缩内容。

公网 IP 检查与 DNS 固定行为均在服务端执行，界面校验不能替代服务端边界。部署需要 API 能直接访问公网 DNS 与 HTTP/HTTPS，应用抓取不使用构建时代理配置。

## 正文与架构

`personal-ai-knowledge::web::WebImporter` 定义端口，`crates/web-import` 用 reqwest 和 scraper 实现，应用入口注入具体适配器。端口使 HTTP 路由测试可以注入固定网页，而无需放开生产内网访问。

正文优先取首个可见 article，其次 main，最后 body；保留块级段落，过滤 script、style、导航、页眉页脚、表单、嵌入对象、hidden 和 aria-hidden 内容，不运行 JavaScript 或加载子资源。采用后台阻塞任务解析，原文最多 1 MiB 且少于等于 20000 个 `<` 字节；请求超时/取消后，已开始的 HTML 解析可能继续，但并发许可直到解析结束才释放。

这不是浏览器排版引擎或完整 Readability：不计算 CSS，不支持依赖登录、JavaScript 的正文，不含 GBK 等编码转换、OCR 或 HTML 文件下载。多文章页面只选择首个 article，复杂页面可能保留杂项或遗漏内容；页面标题与正文属于不可信输入，React 以文本显示。

参考：[reqwest 客户端配置](https://docs.rs/reqwest/0.12.28/reqwest/struct.ClientBuilder.html)、[scraper HTML 解析](https://docs.rs/scraper/0.27.0/scraper/)。

## 持久化与迁移

追加 `0006_web_documents.sql`，增加 `original_html` 和 `web_page` 类型，更新格式约束。Markdown/PDF 行保持现有字段及原文件规则；网页原 HTML、正文、分块和元信息在同一个 INSERT 中原子保存，暂存在 PostgreSQL，MinIO 仍是后续工作。所有列表/详情查询继续按用户隔离。

## 验证范围

- 适配器测试：常见 SSRF 地址、十进制/十六进制 IPv4、混合公网/内网 DNS 集合、正文过滤、Unicode/实体、真实本机 HTTP 的 DNS 固定、无凭证请求、重定向禁止自动跟随、非 HTML/错误编码/超大流拒绝。
- API 测试：注入固定网页验证认证/CSRF 前不抓取、正文类型互斥、默认标题、最终来源、分块、去重、用户隔离、抓取失败不新增文档、原 HTML 不进入 JSON。
- PostgreSQL 测试：迁移、重新连接后的 HTML/正文完整持久化和用户隔离。
- 双入口 Chromium：真实 API 拒绝私网与非默认端口、错误提示、页面重载计数保持为零；继续执行既有 Markdown/PDF 导入回归。

默认自动测试不依赖外部网站稳定性；本机 HTTP 测试只在私有测试函数中注入连接地址，不向生产配置增加任何内网放行开关。

`make browser-test-public` 显式启用真实公网浏览器验收：Next.js 桌面和 Nginx 窄屏分别经页面提交 `https://example.com/`，验证默认标题、来源链接、正文和分块、概览自动更新、输入重置、刷新后持久化、重复导入 409、跨用户详情 404 和另一用户独立导入。API 使用生产抓取器直连公网，测试不拦截导入请求。截图写入各测试的 `public-web-import.png`。该模式要求 API 容器能访问公网，网站内容变化或网络故障均会使测试失败；普通 `make browser-test` 跳过这两项用例。

### 2026-09-15 本地结果

- `make check` 通过（含 10 项 API 测试、7 项网页适配器测试）；回环 HTTP 测试在允许监听本机端口的执行环境运行。
- 前端 lint、typecheck、build 通过。
- `make browser-test` 的 2 项 PostgreSQL 测试、全部 HTTP smoke、8 项 Chromium 测试通过（25.9 秒）。已检查桌面与窄屏网页表单截图，测试容器、网络和数据库已清理。
- 补充的 `cargo test -p personal-ai-web-import live_public_html_import -- --ignored` 未通过：本机无法解析 `example.com`，独立 `curl --noproxy '*' --head https://example.com/` 同样返回 `Could not resolve host`。该测试默认忽略，不作为离线 CI 依赖；公网成功抓取和浏览器成功导入验收仍待网络恢复后完成，不计为已通过。
- 浏览器回归之后补充的隐藏祖先过滤/URL 规范化长度检查已通过最终 `make check`，来源链接换行样式已通过最终前端检查和构建；未为这两项修改重复全套 Docker 验收。

### 2026-09-16 macOS / OrbStack 验收

- `make browser-install` 安装 mac-arm64 Chromium 成功，独立无头启动检查通过；运行不需要前台点击或键鼠操作。
- `make check` 的 23 项常规 Rust 测试、前端 lint/typecheck/build、Compose 配置检查通过。单独执行此前 ignored 的 `live_public_html_import` 公网适配器测试通过。
- `make browser-test-public` 通过：2 项真实 PostgreSQL 测试、完整 HTTP smoke（含数据库/API 重启与会话持久化）、10 项 Chromium 测试全部成功，UI 阶段耗时 38.4 秒，未跳过用例。
- 两项新增公网测试分别通过 Next.js 桌面与 Nginx 窄屏入口，使用真实公网抓取器和数据库；已检查各自 `public-web-import.png`，中文、来源链接及提取正文显示正常。
- 普通模式已单独验证会跳过这两项公网用例（在启动浏览器之前），保持默认 CI 不依赖外部网站。
- 新机首次使用旧构建器下载/构建累计触发 20 分钟命令上限，自动清理成功；保留插件路径后重跑使用 buildx 完成验收。两个测试项目的容器、网络及数据库卷均已清理，镜像/浏览器/依赖缓存保留。未验证远程 CI。
