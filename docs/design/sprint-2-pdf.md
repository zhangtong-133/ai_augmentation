# Sprint 2：PDF 导入

后续更新：原文对象存储现已实现，启用方式、兼容规则与跨存储事务边界见 [MinIO / S3 原文存储](sprint-2-object-storage.md)。以下保留本阶段交付时的设计。

## 行为和接口

Dashboard 文件框支持 Markdown（UTF-8、256 KiB）和 PDF（5 MiB）。PDF 服务端提取文本后按原有 Unicode 规则每块最多 1000 字符分块；PDF 中的 Markdown 符号保留为普通文本。成功后刷新文档列表和今日概览，详情展示「PDF 提取文本」。不包含 OCR、原文件下载或页面布局还原；扫描件无文字时提示先 OCR。

`POST /api/documents` 保持 JSON 协议、Cookie 会话及 `X-Requested-With: personal-ai` 检查。`title/source/tags` 规则不变，`markdown` 与 `pdf_base64` 必须恰好提供一个非 null 字段；后者为标准 Base64 编码 PDF。服务端校验文件头与解析结果，不信任文件后缀和 MIME。API、Next.js 代理及 Nginx 的请求上限统一为 8 MiB（包含 Base64 开销）；Markdown 正文仍限 256 KiB。

列表和详情增加 `source_type: markdown | pdf`。为兼容现有调用方，详情的 `markdown` 字段在 PDF 情况下装载提取的文本。原始 PDF 不在 JSON 中返回。PDF 去重键为 `pdf:` 加原始字节 SHA-256，同一用户重复原文件返回 409；不同用户可以各自导入。不同 PDF 即使提取文字相同也视为不同文档。

## 解析和部署

外部实现隔离在 `crates/pdf-poppler`；应用入口的导入编排调用适配器，知识分块 crate 不依赖 Poppler。

适配器通过 stdin/stdout 调用 Linux `prlimit -- pdftotext`，不创建临时文件、不接收用户命令参数、丢弃 stderr，不记录正文或凭证。依据 [Poppler pdftotext 手册](https://manpages.debian.org/bookworm/poppler-utils/pdftotext.1.en.html) 采用 UTF-8、关闭分页符；退出码非零拒绝导入。无法读取的加密 PDF 与损坏 PDF 提示检查密码或文件。复杂排版/字体可能影响提取顺序或准确性。

每进程最多两个解析任务，超出立即返回 503；每个子进程限制 512 MiB 虚拟内存、5 秒 CPU 和 7 秒墙钟时间。输出最多 1 MiB UTF-8 文本，读取越界即终止；取消请求时 kill-on-drop，正常失败路径杀死并回收进程。不会部分导入超限文本。并发限制为单个 API 实例的限制。

Compose 的 Rust 运行镜像安装 `poppler-utils` 和 `util-linux`。本机直接运行 API 需要这两个 Linux 包，缺失时 PDF 导入返回 `pdf_unavailable`，Markdown 仍可用。macOS 原生进程未适配 `prlimit`，请使用 Compose。

## 数据迁移

追加 `0005_pdf_documents.sql`，既有文档默认 `source_type=markdown`。PDF 原文件暂存 PostgreSQL BYTEA，与元信息、提取文本和分块在同一 INSERT 中原子保存；数据库约束要求 PDF 原文件存在且不超过 5 MiB。列表查询不读取二进制。后续 MinIO 适配器工作再迁移原文存储；此轮不要求 MinIO。

## 验证

- Rust 路由回归涵盖认证、CSRF、混合/缺失正文、非法 Base64 和假 PDF。
- PostgreSQL 集成测试验证新增字段、PDF 原字节和文本持久化、跨用户不可读。
- 双入口 Chromium 验收覆盖超过 2 MiB PDF 上传、字面文本/分块、重载、去重、用户切换和独立导入、损坏 PDF、无文字 PDF、5 MiB 文件上限、1 MiB 提取文本上限、接口不返回原始二进制；原 Markdown 验收继续运行。
- 执行 `make check`、前端 lint/typecheck/build、`make browser-test`。真实 Poppler 验收在隔离生产镜像中进行；本机无 Poppler 不影响纯 Rust 单元测试。

### 2026-09-15 本地结果

`make check`（rustfmt、Clippy warnings denied、Rust 全量测试）、前端 lint/typecheck/build 全部通过。`make browser-test` 的 2 项 PostgreSQL 测试、全部 HTTP smoke、6 项 Chromium 测试通过（UI 17.3 秒）；桌面和窄屏 PDF 详情截图已人工检查。全部本轮隔离数据库、容器和网络已清理。

首次运行镜像构建因 apt 直连下载缓慢中止重试，最终改用 Debian 官方 HTTPS 源、构建镜像的 CA 证书和兼容 apt 的小写代理变量；未关闭 TLS 校验，也未把代理配置保存到运行时环境。
