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

## 第二阶段：独立门槛与逐场景诊断

```sh
node scripts/local-value-public.mjs 校准1.json 校准2.json 留出1.json 留出2.json
make local-value-public-test
```

门槛只接受当前 v4、当前冻结 manifest、相同模型和运行文件指纹的完整套件报告；两套各至少两轮、所有提供轮次质量通过且重复一致才通过。重复文件、重叠运行、来源/标签/请求改变、混合旧套件均拒绝。新增通过轮不能覆盖已提供的失败轮；原三套合成门槛和换序门槛继续独立保留，不自动启用业务。

私有报告 `rss-public-material-gate-v1` 固定声明代理预标注、尚未人工复核、真实材料质量未确认。每套分别输出比较、覆盖率和四个场景诊断；Rust/Python/Linux 主题与 partial/关键词改写探针分开。诊断单位为条目 × 轮次：

- `required_score_rate` 只在已观察到的要求给分条目中统计给分比例，并同时展示 completion_rate；0 是给分，null 是弃权。
- `unexpected_abstentions`、`below_minimum`、`above_ceiling` 分别统计弃权、低于下限、超过上限。数值错误不与弃权重复计数。
- 对 minimum=ceiling=40 的预标注条目，另记 partial 正确、高估、低估及弃权。
- 失败场景和后续未运行条目单独计数，分母为零时给分率为 null，不编造成满分或零分。

“错误”仅相对于预先保存的标签，不说明标签已客观验证；不得用各主题总通过率掩盖某一个探针的失败。此阶段仅离线工具和测试集成，尚未变更业务权限、数据库或页面。

第二阶段工程验收：5 类新门槛测试、9 类比较器、6 类原门槛及5 类换序回归通过；真实 CLI 正确拒绝重复报告和旧套件，且不回显原报告。生产镜像/core smoke 通过，包含 170 项 PostgreSQL、41 项学习 HTTP、6 项评分 HTTP、6 项 Redis 以及双代理核心验收；镜像内两个公开材料 manifest 验证通过。临时测试栈已清理。Rust 与前端沿用第一阶段结果，此阶段未修改这些实现；无页面变化，未重跑浏览器专项。

## 第三阶段：真实四轮结果

默认官方 Qwen3-4B Q4_K_M、llama.cpp b11382、8192 上下文、v4 协议，按预定校准两轮、留出两轮执行；全程没有改提示、模型、语料或标签。每轮协议完整，无未运行场景，两个套件各自分数/检查完全重复，但新质量门槛未通过：

| 套件 | 每轮通过场景 | 两轮要求给分条目 | 实际给分 | 弃权 | 低于预标下限 | 超过预标上限 |
| --- | --- | --- | --- | --- | --- | --- |
| public_calibration | 3/4、3/4 | 24 | 22 | 2 | 2 | 0 |
| public_holdout | 1/4、1/4 | 24 | 16 | 8 | 0 | 0 |

校准集三个主题场景均通过；`memory_partial` 中 Rust=80、Linux=0、Python=null。相对预标注，Linux 从期望的 partial=40 变为 0，Python 无关但内容充分却弃权。partial 的“间接相关”边界本身尚待独立人工复核，不能只凭代理标签断言该结果客观错误。

留出集 `python_topic` 通过；`rust_topic` 对 Python 文档弃权，`linux_topic` 对 Rust/Python 文档均弃权，`borrowing_paraphrase` 对 Python 文档弃权。各场景直接相关文档均为 80，已给出的无关数值均为 0，未出现超过预标上限的分数。本批主要暴露“无关”和“材料不足”的混淆，不证明跨主题完整判定能力，更不能因无错误高分就将整体门槛算作通过。

私有证据（`.local-model/` 下，均已验证 0600 权限）：

- 校准第一/二轮：`quality/run-XXXXXXJ2tQUL/report.json`、`quality/run-XXXXXX77TcM2/report.json`，均退出 2。
- 留出第一/二轮：`quality/run-XXXXXXKYX69V/report.json`、`quality/run-XXXXXX1m3mOF/report.json`，均退出 2。
- 正式离线门槛：`quality/run-XXXXXX9Q8uMY/report.json`，`public_fixture_gate_passed=false`，退出 2。报告完整保留两个套件的失败与分主题计数，没有补跑通过轮替换失败轮。
- 显存观测：`observations/run-XXXXXXmnInkB/report.json`，180 秒、332 个样本，最低空闲 11367 MiB（约 11.10 GiB）、采样最高占用 4936 MiB、最大采样间隔 674 ms；低于 6144/6656 MiB 的样本均为零。时间窗口已核对覆盖四轮请求。期间同时进行镜像构建，未将延时当作模型性能基准。

模型已显式停止，supervisor 状态清理，停止后空闲 14666 MiB；观测末尾为 14665 MiB。离散采样不保证捕获瞬时峰值，未运行游戏。保持业务开关、默认模型及 v4 提示不变；本批未调用 8B、vLLM、付费 API 或真实用户数据。未重跑浏览器、真实本地学习恢复、S3/向量可选服务或公开部署；本阶段只是实测与文档，工程检查沿用前两阶段。

## 后续任务

1. RSS：先明确并复核“有充分内容但主题无关”和“内容不足”的标签边界；partial 单独记录人工判断及分歧。再设计新的候选协议，在校准集上评估，不修改 v4 或修补原输出。候选必须回归原四套及当前材料集，另冻结未用于调参的新文档留出集；已查看成绩的本次留出集今后只作为回归证据。
2. 私有问答基础：现有接口已经校验检索命中的所有者并限制引用 id。下一步补生成完成后的来源与会话复核，验证生成期间删除文档、撤销会话时不返回旧正文；这项接口工作不依赖 RSS 分类分数。
3. 私有问答证据：在既有引用 id 上扩展原文范围与逐段引用校验，再接精确分享预览、一次授权及供应商无关本地适配。基础工程完成与模型回答质量仍分别验收，不以 RSS 门槛替代问答质量测试。
