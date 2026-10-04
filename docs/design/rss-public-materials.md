# RSS 公开文档改写材料与分主题验收

## 实施阶段

1. 在模型调用前冻结校准集、留出集、来源及代理预标注条件；不改原四套语料或业务 v4 提示。
2. 交付独立双套件双轮门槛，逐主题记录有效给分、弃权、错误高分与 partial 档位偏差。
3. 受至少 6 GiB 显存保护运行默认 4B 四轮，保留全部结果，停止模型并记录显存观测。每阶段独立验收、提交、推送。

## 第一阶段：来源与条件冻结

`public_calibration` 和 `public_holdout` 各 4 场景，每套复用 3 篇文档摘要，一共 6 个独立文档 URL。所有正文都是代理阅读官方文档后的短篇中文改写，不是原始 RSS，不是原网页全文快照；访问核对日期为 2026-10-04。固定语料仍标记 `synthetic_only=true`，新增 manifest 字段 `material_origin=public_document_paraphrase` 进一步区分来源；旧四套 manifest 不增加字段。

校准集来源：

- [Rust 所有权](https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html)：Ownership Rules、Memory and Allocation。
- [Python venv](https://docs.python.org/3/library/venv.html)：Creating virtual environments、How venvs work。
- [Linux cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html)：What is cgroup、CPU、Memory。

留出集来源：

- [Rust 引用与借用](https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html)：共享/可变引用及悬垂引用；摘要较长。
- [Python asyncio 协程与任务](https://docs.python.org/3/library/asyncio-task.html)：Coroutines、Awaitables、Task groups。
- [Linux ext4 高层设计](https://docs.kernel.org/filesystems/ext4/overview.html)：块组和数据布局。

留出只保证文档 URL 不重叠，不保证出版物、领域或模型训练数据独立。两套在本批均预先冻结，后续不能一边看留出成绩一边改提示，再宣称它仍是未见留出集。

两套各含 Rust、Python、Linux 三个主题场景，要求对应摘要至少 60、其他摘要同时 minimum 0 与 ceiling 20，避免无关内容靠弃权通过。校准集另加 `memory_partial`：目标“编程语言内存管理”，所有权为直接相关，cgroup 的操作系统内存限制预标为间接相关（40），venv 为无关。留出集另加 `borrowing_paraphrase`，用“共享引用/可变借用”代替 Rust 名称验证概念关键词。所有阈值和理由均在模型调用前保存。

`annotation_method=agent_prelabelled_unreviewed` 明确说明标签由代理拟定，尚未经独立人工复核，尤其 partial 的相关性边界存在主观性。此批不宣称已完成人工标注或真实用户材料验收。模型质量未通过时仍保留原条件，不将候选成绩自动用于业务启用。

语料文件只存一份各源摘要，由固定 case 引用生成业务请求。载入器拒绝未引用/未知/重复来源、跨集同 URL、非允许官方域名或伪造人工复核标记；来源、日期、预标注理由和完整摘要均绑定到 `corpus_sha256`，实际发送内容另有请求指纹。模型只看到标题/摘要/关键词，不收到 URL、标签、标注理由或预期分数；运行不访问网络、数据库或用户订阅。

## 验收与边界

第一阶段的代码验收与后续真实调用结果按阶段追加。默认 4B、v4 业务提示、原开关值和供应商无关推理接口保持现状。vLLM、真实游戏压力、用户私有材料，以及私有问答的来源引用与撤销隔离另行验收。

第一阶段验收：17 项领域质量测试、9 类报告比较器、5 类基准、6 类原门槛、5 类换序验收，以及全仓 Rust fmt/Clippy/tests 和前端 lint/typecheck/build 均通过。原四套当前 manifest 与已保存真实报告逐字段一致；新两套最大请求分别为 2031/2375 字节，均低于 5632 字节预算。此阶段未调用模型、未改数据库/API/页面，PostgreSQL、生产镜像和浏览器专项未重跑，镜像打包留到第二阶段验收。
