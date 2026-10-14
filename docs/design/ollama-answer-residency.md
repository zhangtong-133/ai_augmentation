# Ollama 驻留复用与资源保护

2026-10-14：[单题阶段诊断](ollama-answer-diagnostics.md)已增加证据一致性及类型/支持汇总计数。两次实际请求及驻留策略保持，诊断使用独立报告，不计入下述完整质量门槛。

2026-10-12：用户说明已有 Ollama 模型，并询问每次是否需要卸载。逐次冷加载是此前合成评估的固定条件，留出游戏显存本身不要求逐次卸载。本批增加有限驻留复用，继续保护资源余量。

## 使用

```bash
make ollama-answer-benchmark-preview CASE=single_fact
make ollama-answer-benchmark SUITE=baseline
make ollama-answer-diagnose CASE=single_fact

# 原 v14 冷加载对照仍可显式选择
make ollama-answer-benchmark-preview CASE=single_fact RESIDENCY=cold
make ollama-answer-benchmark SUITE=baseline RESIDENCY=cold
```

宿主入口默认 `--residency reuse`，使用 `ollama-knowledge-answer-v15-reuse`。每次原生请求固定 `keep_alive=30` 秒：连续请求之间保留权重，最后一次请求结束后由 Ollama 的空闲计时器释放。已驻留的相同模型可以直接进入评估，不要求先卸载。调用会设置该模型的空闲期限；若它原先长期驻留，本次请求后也采用 30 秒期限。模型文件保留，Ollama 服务继续运行。

`--residency cold` 保留 `ollama-knowledge-answer-v14`、`keep_alive=0`、每次发送前空闲检查及自动卸载等待。底层 Rust CLI 为兼容旧冻结清单仍默认 cold；宿主显式传入所选策略。两种模式的预览、manifest 和真实发送必须一致，不接受任意驻留时间、无限驻留或 llama.cpp 的驻留参数。

该入口仍仅使用内置合成资料和已安装模型。业务默认 llama.cpp v2、已有分享/本机计算授权及私有执行关闭状态保持。PC 的 NVIDIA 至少 6 GiB 显存余量、512 MiB 缓冲、运行监督和空闲释放继续由原部署入口实现；游戏并行/FPS 观察为可选项。

## 每次发送前的资源检查

两次独立推理继续分别等待宿主许可，第一步结果不进入第二步输入。语义提示、JSON schema、原始资料、8192 上下文、2048 输出、采样参数、严格流终态和原文引用规则与 v14 相同；两个请求仅将 `keep_alive` 从 0 改为 30，并绑定新 profile/实际请求指纹。没有推理重试。

宿主每次重新核对 `/api/version`、`/api/tags`、`/api/show` 的已安装模型身份；随后读取 `/api/ps` 并分类：

| 状态 | 条件 | 发送前估计余量 |
| --- | --- | --- |
| cold | 没有驻留模型 | 6 GiB + 512 MiB + 1.5 倍模型文件大小 |
| reuse | 唯一驻留模型的别名、digest、GGUF 和运行器一致，上下文为 8192，空闲期限至少还有 10 秒 | 6 GiB + 512 MiB + 1 GiB 推理缓冲 |
| reload | 同一模型，但上下文不同、期限不足或运行器字段缺失 | 完整 cold 加载预算 |

Ollama 0.40.3 实测 `/api/ps` 的 `runner` 在顶层；兼容从 `details.runner` 读取。两个字段如同时出现都必须匹配。其他模型、其他运行器、多个驻留模型、身份变化、非法/缺失资源元数据拒绝发送，不为本次评估卸载其他模型。

内存压力最多等待 15 秒，只重新查询资源。资源检查后再次读取驻留状态，若 cold/reuse/reload 分类变化则拒绝本次许可，防止按热预算确认后模型已消失。取消、超时、查询失败均不发迟到许可。已驻留模型仍需额外 1 GiB 缓冲，不能仅通过“模型已经加载”跳过资源检查。

推理期间每 500 ms 查询统一内存估计，低于 6 GiB + 512 MiB 或查询失败时取消本次子进程和连接，拒绝后续请求。该检查不强制停止共享 Ollama 服务，也不发送强制卸载请求。取消连接不保证服务立即停止计算；权重释放依赖服务完成/空闲计时器，其他客户端请求可能延长驻留。

`/api/ps` 只能观察驻留，不能证明其他客户端没有正在推理。项目锁仅串行化本项目评估，不接管共享服务。Apple 统一内存估计、加载系数和缓冲均为软件预算，不是 PC 显存硬隔离或分配保证；发送与采样间也可能出现外部占用变化。

## 冻结与报告

八套全部 71 道条件与资料沿用 v14。第一次 v15 推理前，在 `2026-10-12T07:11:15.591Z` 冻结新实际双请求指纹：

`apps/api-server/tests/fixtures/answer-residency-manifests-ollama-v15.json`

文件 SHA-256：`b8ecd0405963ff18fcd1be464f2216d015021c2d9da918b1009bd128106ab2f0`。新文件只保存 71 个请求指纹及驻留条件，完整语料/条件由原 v14 清单逐字段核对；原 v14 文件及历史失败保留。预览单元测试同时核对两个请求仅改变有限驻留时间。

复用报告为 `ollama-answer-quality-report-v3`、资源策略为 `macos-pressure-reserve-6gib-buffer-512mib-load-1.5x-reuse-1gib-idle-30s-v1`。除了每题两次资源检查，还分别记录 cold/reload/reuse 次数和加载/复用阶段最低余量；没有观察某阶段时对应最低值为 null，不虚构加载记录。模型正文、所选 key 和逐项模型状态均不保存。

新门槛 `ollama-answer-quality-gate-v9` 仍要求八套各两份不同、顺序执行的完整报告、全部原条件和同一运行时。它核对分类计数之和、阶段预算及实际复用观察，不允许和 v14 冷加载报告混合；单题诊断和未完成报告不能进入门槛。旧 v2 报告和 v8 门槛仍可离线重算。两类门槛都不授予私有执行权限。

## 验收记录

第一次单题诊断因误从 `details.runner` 读取运行器，把实际已驻留模型归为 reload，完整加载预算不足，拒绝第二次请求；只发送第一步，无自动重试。第二次元数据诊断确认本机 `runner` 为顶层字段并保留同样的未完成记录。随后修复字段识别，增加顶层、嵌套及冲突字段测试。两份诊断均不计入完整质量报告。

Ollama 0.40.3 / Qwen3.5:9B / llamacpp / macOS ARM64 32 GiB 的完整八套各两轮已经完成。模型 digest `c97eb11d70b1acdc88af01eef566c1fe4f7fbe93eb1afc06871132f293ff425a`、文件 6603314868 字节、元数据 SHA-256 `bc5dfe465ee401a890deef941fefd3e9bfe18c3ddeeb814b3c1ceed2ee67c3be` 与此前 v14 相同。

| 套件 | 第一轮 | 第二轮 | 与同运行时 v14 的逐项检查 |
| --- | --- | --- | --- |
| baseline | 5/7 | 5/7 | 一致 |
| challenge | 7/8 | 7/8 | 一致 |
| coverage | 10/10 | 10/10 | 一致 |
| extraction | 7/8 | 7/8 | 一致 |
| decision | 4/4 | 4/4 | 一致 |
| mixed | 10/10 | 10/10 | 一致 |
| availability | 12/12 | 12/12 | 一致 |
| support | 10/12 | 10/12 | 一致 |

142 道完整双请求答案均通过原生协议/生产引用校验，质量仍为 130/142，568 个布尔检查与 v14 逐题、逐轮一致。原有六题各两轮失败全部保留；新 v9 门槛退出 2、`passed=false`、`private_execution_authorized=false`，没有以复用策略掩盖质量失败。

284 次发送前资源许可分类为 1 次 cold、283 次 reuse、0 次 reload；后续轮次可以直接使用上一轮仍驻留的同一模型。加载前最低估计余量 19.52 GiB，复用前及运行中最低 11.20 GiB，另有 2280 次运行采样。结束时模型仍驻留，随后约 30.28 秒的只读观察确认自动释放；未发送卸载命令，服务保持在线，项目锁释放。

每题完整双请求（含宿主资源许可）的耗时中位数为 7.496 秒，P95 为 10.823 秒，范围 5.610–13.495 秒；同运行时此前 v14 冷模式为中位数 11.855 秒、P95 15.344 秒，范围 10.114–17.516 秒。这是同一固定合成集的分时记录，不是 PC 游戏或真实用户负载性能验收。

独立审计逐字段核对全部冻结请求、原条件、运行时、16 份不同报告、阶段计数、预算及 700/600 目录/文件权限；重算新 v9 与原 v14 的 v8 门槛都与原结果一致，随后读取 `/api/ps` 确认为空。报告仍只在本机 Git 忽略路径，未保存模型正文或所选 key。

工程验收：默认 Rust 1.96 与 CI 对齐的 Rust 1.99 分别完整通过 fmt、Clippy `-D warnings` 与 329 项测试，286 项需指定真实服务的用例在默认工作区测试中忽略；Node 公共问答 4 项、Ollama 17 项、PC 显存保护 6 项共 27 项通过；前端 lint/typecheck/build 与 11 项单元测试通过；隔离 core smoke 281 项真实服务测试和生产镜像全部 71 道冷热清单、双请求保留时间预览通过。已独立核对本次 `personal-ai-smoke-40a16f1df373ab14` 的容器、网络、测试数据卷均移除，只保留构建镜像缓存。

跳过浏览器 UI、Qdrant/index、MinIO、真实公网材料、游戏并行/FPS、vLLM 及私有问答执行验收。本批没有 UI、迁移或业务开关变化。上一轮提交 `721989f` 的远程 CI 已核对为 Rust、Web、smoke、index、objects 全部成功；本批 CI 以推送后的实际运行结果为准。

## 后续

驻留策略不修复原有事实选择/逐项支持质量问题。继续按[独立语义核对](ollama-answer-independent.md)增加受限的合成阶段诊断，保存所有失败与对照。私有执行仍需完整质量通过、实际双请求精确预览/同意和一次性执行器，不能复用原 v2 授权。

官方说明：[模型驻留与 `keep_alive`](https://docs.ollama.com/faq)、[驻留模型元数据](https://docs.ollama.com/api/ps)、[原生聊天请求](https://docs.ollama.com/api/chat)。
