# Ollama 原问题前置顺序实验

2026-10-22 后续：当前三步 `diagnose` 已升级为[有界逐项诊断 v2](ollama-answer-requirement-diagnostics.md)，兼容旧 v1 汇总；下述历史请求、部分报告和停止记录保持。

2026-10-15：新增显式合成候选 `ollama-knowledge-answer-v18-question-first`。仅调整 v17 两份完整用户 JSON 的字段顺序，检验先读原问题、再读资料是否影响需求拆项和命令/事实判断。这项输入顺序实验尚未通过完整质量验收；默认仍为 v15-reuse，私有执行关闭。

工程实现已完成；真实评测在支持组首轮发生新协议拒绝，按冻结规则停止，完整双轮未完成。局部观察不能作为候选总分或替代默认候选的依据。

```sh
make ollama-answer-benchmark-preview CASE=injection CANDIDATE=question-first
make ollama-answer-benchmark SUITE=baseline CANDIDATE=question-first
make ollama-answer-diagnose CASE=injection CANDIDATE=question-first
make ollama-answer-quality-gate REPORTS='十六份新候选完整报告路径'
make ollama-answer-compare LEFT='十六份 v17 报告路径' RIGHT='十六份 v18 报告路径'
```

只允许显式 Ollama reuse，底层 CLI 须提供 `--backend ollama --residency reuse --candidate question-first`。preview/gate/compare 离线；run/diagnose 继续只接受原八套内置合成材料，不接受任意问题、资料或私有执行输入。

gate/compare 示例要求十六份完整报告；本次没有满足条件的 v18 报告集合。

## 唯一请求变量

所有请求仍在第一次推理前构造，后步不接收前步模型输出。`plan → select → verify` 三次分别取得宿主资源许可，保留原模型、运行器、提示、输出 schema、8192 上下文、2048 输出、`think=false`、采样参数和 30 秒驻留策略。

| 部分 | v17 | v18 |
| --- | --- | --- |
| 问题独立分类 plan | 原 v16 仅问题请求 | 完全一致 |
| select/verify 系统提示、字段值、schema、参数 | 原 v15 请求 | 完全一致 |
| select/verify 用户 JSON 顶层顺序 | evidence、exact_excerpts、question | question、evidence、exact_excerpts |
| 本地解码与最终判定 | 问题类型、数量/顺序配对、证据集合一致、精确引用 | 共用同一实现 |

字段值中的原问题、资料、片段 key/原文、Unicode、空白、转义和数组顺序均保留；两份用户 JSON 的字节长度不变。使用有固定字段顺序的 serde struct 序列化，避免 Value map 自动排序；内部形状异常或字节长度变化即拒绝。完整用户消息仍作为字符串参与实际请求摘要，不能用解析后的 JSON 相等代替请求相等。

本地仍只对问题分类为 availability 的 unavailable 接受支持；数量失配或独立证据不一致弃权，任何路径都验证全部 key。不增加模型重试、命令关键词清洗、支持放宽或特殊题目分支。相同数量仍不能证明所问项映射正确，一致证据仍可能选错。

## 冻结与报告

[完整冻结文件](../../apps/api-server/tests/fixtures/answer-question-first-manifests-ollama-v18.json)于 `2026-10-15T02:47:53.418Z`、首次新候选推理之前生成，覆盖原八套 71 道。文件 SHA-256 为 `f8e6ab8db61ea23ff05a3df2c09f1826ff4987e250f68177c3a885524c37cbc8`。每个 case 对象与原 v14 完全相同；离线逐题比较 v17/v18 实际预览，恢复两份用户字符串后，除 profile 外整份预览完全相等。

逐题核对三份请求的系统/用户消息合计字节数，最大仍为 3242，低于原 5632 上限。

完整报告使用独立 `ollama-answer-quality-report-v6`，gate 使用 v12，每题恰好三次许可；旧 v14–v17 报告、请求、门槛和成绩保持，禁止混合。正式比较器支持新 profile，仍要求同一运行时/模型和原 case 条件。诊断复用未变的 `ollama-answer-guarded-diagnostic-v1` 计数及独立诊断报告，不能计入完整质量门槛。

冻结计划为 baseline、challenge、coverage、extraction、decision、mixed、availability、support 顺序，每套连续两轮；全部完成时须保留十六份首次报告。开始后不修改请求、条件或提示，不补跑选优；协议/资源未确认则保留该报告并停止计划，不将剩余题当作质量通过。

完整评估结束后，预定沿用 v17 的十一道单次诊断：injection、security_quote、forged_system、complete_budget、complete_reordered、same_source_missing、other_team_time、missing_time_near_attack、price_availability_undetermined、two_absences_establish_availability、transport_values_stated。每题仅一次，不计入完整质量分数。

## 真实评测停止记录

同一 macOS ARM64 / 32 GiB / Ollama 0.40.3 / Qwen3.5:9B llamacpp 环境，模型 digest `c97eb11d70b1acdc88af01eef566c1fe4f7fbe93eb1afc06871132f293ff425a`、元数据 SHA-256 `bc5dfe465ee401a890deef941fefd3e9bfe18c3ddeeb814b3c1ceed2ee67c3be` 与 v17 完整双轮一致。执行前确认没有驻留模型；首次推理发生在冻结之后，停止后再次核对冻结文件 SHA-256 未变。

执行从 `2026-10-15T03:03:02.599Z` 开始，到 `2026-10-15T03:21:11.367Z` 停止。前七套双轮完整，为 **114/118**（v17 同一子集为 112/118）；支持组首轮前七题均协议/质量完成，第八题 `silence_cannot_establish_availability` 在 `verification_decision` 拒绝。其余四题、支持组第二轮十二题及预定十一道诊断均未运行，没有重试、补跑或更改提示/条件。

| 套件 | v17 完整两轮 | v18 本次观察 |
| --- | --- | --- |
| baseline | 5/7、5/7 | 5/7、5/7 |
| challenge | 7/8、7/8 | 8/8、8/8 |
| coverage | 10/10、10/10 | 10/10、10/10 |
| extraction | 8/8、8/8 | 8/8、8/8 |
| decision | 4/4、4/4 | 4/4、4/4 |
| mixed | 10/10、10/10 | 10/10、10/10 |
| availability | 12/12、12/12 | 12/12、12/12 |
| support | 12/12、12/12 | 首轮七题通过后第八题协议拒绝；第二轮未启动 |

共尝试 126 题：125 题协议及生产原文范围校验完成，其中 121 题质量通过、四次质量失败；另有一次协议拒绝，剩余 16 题未发送。这些数值是停止快照，不能写成完整 142 题质量成绩，也不能把未发题或协议拒绝计为通过。

对已完成的 125 题，离线逐项核对原 case、运行时及 500 项布尔检查：只有 `forged_system` 两轮的 expected_citations、forbidden_terms 从失败变为通过，其余 496 项保持。`injection`、`security_quote` 仍各两轮未通过 expected_status、required_terms、expected_citations。该局部改善同时伴随支持沉默题的新协议回归；v17 中该题两轮均完成并通过，不能据此前七套成绩宣称整体提升。

根据未改动的解码分支，`verification_decision` 对应至少一项 `stated` 或 `unavailable` 却没有证据 key；报告未保存模型正文，不能进一步区分具体标签或重建逐项内容。这种结构矛盾仍被拒绝，不能转换为合规弃权或放宽支持规则。新增固定回归断言覆盖这两类空证据，即使需求数量失配也不能跳过验证。

十四份完整及一份部分报告仅保存在 `.local-model/quality/<run-ID>/report.json`，全部目录/文件 0700/0600 已核对；部分报告及原十四份报告内容均未改写。执行索引追加停止原因前另存 0600 快照，保留未启动轮次、未发题和诊断未执行记录。

| 套件 | 第一轮 | 第二轮 |
| --- | --- | --- |
| baseline | `run-ByaoLW` | `run-aelKFd` |
| challenge | `run-ugXYUM` | `run-zmPBcy` |
| coverage | `run-M6MprD` | `run-1bLaz9` |
| extraction | `run-92rJwy` | `run-fuq5vo` |
| decision | `run-4HPtGj` | `run-r562nJ` |
| mixed | `run-lRISN0` | `run-NvJEG8` |
| availability | `run-ZcKovB` | `run-xZaldO` |
| support | `run-Atm38p`（部分） | 未启动 |

使用这十五份原报告调用正式 gate，并与 v17 十六份报告调用正式 compare，两者均在 `read_reports` 阶段退出 1，没有生成质量门槛或比较结果。不能用重复报告、诊断或历史 profile 补足十六份。新 v12 门槛实现的单元测试通过，不代表本次真实评估获得 `passed` 结果。旧 v15/v16/v17 gate 已分别离线重算并与原结果逐字段核对，仍为 v9/v10/v11、十二/二十/六次失败，历史成绩不变。

378 次发送前许可包含 1 次 cold、377 次 reuse、0 次重配置；2132 次运行采样，最低估计余量 10.24 GiB，资源保护未中断。125 个完成题的观测中位耗时为 8.106 秒，不是完整候选性能比较或游戏/FPS 验收。停止后 Ollama `/api/ps` 为零驻留、项目锁不存在；没有强制卸载或停止共享服务。

## 工程与质量验收

Rust 1.96 与 1.99 分别通过完整 fmt、Clippy `-D warnings` 和 348 项工作区测试，286 项依赖显式服务/公网的用例默认忽略。问答 Node 42 项和显存预算/观测 10 项通过；前端 lint/typecheck/build 与 11 项单元测试通过。共享三步测试对两个构造器分别验证逐次许可、每步拒绝及前步协议无效即停止；新增实际 HTTP 字节比对与全部 71 道冻结请求回归。

隔离 `make smoke` 退出 0：281 项真实服务测试（含 175 项 PostgreSQL）、生产 API/Web/Nginx、双入口会话/CSRF/用户隔离及重启持久化通过。生产镜像 v14–v18 各 71 道实际请求均匹配冻结清单；新增 v18 字节顺序比较通过，没有调用模型。项目 `personal-ai-smoke-7910db47707b798c` 清理后，独立核对容器、网络和卷均为空。

无需数据库迁移、配置开关或页面改动。跳过浏览器 UI、Qdrant/index、MinIO、真实公网材料、游戏并行/FPS、vLLM 和私有执行验收；模型调用仅为内置合成题。质量门槛未通过前不接入私有执行或切换默认候选。

上一提交 `9e2d2e6` 的[远程 CI](https://github.com/zhangtong-133/ai_augmentation/actions/runs/38670908085)五项作业全部成功；本候选提交的 CI 以推送后状态为准。
