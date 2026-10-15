# Ollama 支持状态与证据约束实验

2026-10-15：新增显式合成候选 `ollama-knowledge-answer-v19-evidence-bound`。针对 [v18 停止记录](ollama-answer-question-first.md)中的空证据支持矛盾，把未变本地解码器的约束同时表达在核对输出 schema 中。完整双轮协议完成，但质量为 **132/142**，低于 v17 的 136/142；三题各两轮新增质量退步，门槛失败。默认 v15-reuse，私有执行关闭。

```sh
make ollama-answer-benchmark-preview CASE=silence_cannot_establish_availability SUITE=support CANDIDATE=evidence-bound
make ollama-answer-benchmark SUITE=support CANDIDATE=evidence-bound
make ollama-answer-diagnose CASE=silence_cannot_establish_availability SUITE=support CANDIDATE=evidence-bound
make ollama-answer-quality-gate REPORTS='十六份新候选完整报告路径'
make ollama-answer-compare LEFT='十六份 v17 报告路径' RIGHT='十六份 v19 报告路径'
```

只允许显式 Ollama reuse；底层须提供 `--backend ollama --residency reuse --candidate evidence-bound`。run/diagnose 仅接受原八套内置合成材料，preview/gate/compare 离线。没有任意问题/资料入口、模型下载或私有执行接线。

## 受控变化

以 v18 为基线，`plan → select → verify` 三份请求仍在首次推理前构造，逐次核对身份、驻留与资源。planning/selection 原请求及 verification 的完整用户字符串、原提示说明、模型/运行器/采样参数、8192 上下文、2048 输出、`think=false`、`keep_alive=30` 均保持。

仅替换 verification 的 `format` 和系统消息末尾匹配的 schema 声明。每个 check 使用两个自足对象的 `anyOf`：

| support 分支 | evidence 下限 | 其余字段与约束 |
| --- | --- | --- |
| stated、unavailable | 1 | 原字段、kind 枚举、合法 key 枚举、唯一性及上限 |
| unsupported | 0 | 同上，继续允许非空证据 |

没有改变本地解码、按问题类型接受 unavailable 的规则、所问项数量/顺序配对、独立证据集合一致性或原文范围校验。即使运行器忽略 schema，本地仍拒绝 stated/unavailable 配空证据；不将非法结果转为弃权，不重试或按题目分支处理。schema 只约束结构，不能证明所选证据与所问实体、属性相符。

构造时调用原请求校验器检查扩大的系统消息和合计提示预算，再恢复等长的 v18 完整用户原字节。原问题、资料、片段 key/原文、Unicode、空白、转义及数组顺序保持。

[Ollama 官方文档](https://docs.ollama.com/capabilities/structured-outputs)说明可通过 format 提供 schema 并在提示中声明；[llama.cpp 转换器](https://github.com/ggml-org/llama.cpp/blob/master/common/json-schema-to-grammar.cpp)有 anyOf 与 minItems 分支。这些上游资料只支持方案依据，当前安装运行器的实际兼容性仍由本次真实协议评测确认，不预设生成结果正确。

## 冻结与验收计划

[完整冻结文件](../../apps/api-server/tests/fixtures/answer-evidence-bound-manifests-ollama-v19.json)于 `2026-10-15T05:24:03.891Z`、首次 v19 推理前生成，覆盖原八套 71 道。文件 SHA-256 `86fb4e542d596683674a18027ba6e272767fab3a80fa0904b1cecd146fa66348`。所有原 case 对象与 v14 相同；逐题恢复 verifier schema、对应声明及 profile 后，整份预览与 v18 完全相等。

逐题系统/用户消息合计最大 3587 字节，低于原 5632 上限。使用锁定前端依赖中的 Ajv 6.15.0，对实际 71 份 verifier schema 各核对 13 种正反形状（923 次），涵盖合法三状态、非法空支持证据、未知/重复 key、非法 kind/额外字段/null、空或超量 checks；全部符合预期。此离线核对未调用模型。

独立完整报告使用 `ollama-answer-quality-report-v7`，gate v13，每题三次许可。旧 v14–v18 请求、报告和门槛保留；不混合候选或以诊断补完整报告。单题诊断复用原三步有界计数及独立 schema，不能计入完整质量门槛。

固定执行顺序为 baseline、challenge、coverage、extraction、decision、mixed、availability、support，每套连续两轮，共十六份首次报告。开始后不改请求或条件、不补跑选优；任何协议/资源未确认都保留报告并停止计划，不计剩余题为通过。

只有完整双轮结束后，预定六题各诊断一次：baseline 的 injection、security_quote；challenge 的 forged_system；support 的 silence_cannot_establish_availability、price_availability_undetermined、two_absences_establish_availability。若完整计划中断，六题诊断均不执行。

## 真实完整双轮

同一 macOS ARM64 / 32 GiB / Ollama 0.40.3 / Qwen3.5:9B llamacpp 环境，模型 digest `c97eb11d70b1acdc88af01eef566c1fe4f7fbe93eb1afc06871132f293ff425a`、元数据 SHA-256 `bc5dfe465ee401a890deef941fefd3e9bfe18c3ddeeb814b3c1ceed2ee67c3be` 与 v17 完整双轮一致。执行前确认无驻留模型，从 `2026-10-15T05:38:09.417Z` 到 `2026-10-15T05:58:11.163Z` 完成全部首次报告。没有重试、选优补跑或修改请求、题目条件；142 题均完成协议和生产原文范围校验，没有未运行题。

| 套件 | v17 两轮 | v19 两轮 | v19 首轮 / 次轮报告目录 |
| --- | --- | --- | --- |
| baseline | 5/7、5/7 | 5/7、5/7 | `run-2bUcHP` / `run-offl3s` |
| challenge | 7/8、7/8 | 8/8、8/8 | `run-0bZ3WZ` / `run-L4nl64` |
| coverage | 10/10、10/10 | 10/10、10/10 | `run-NkmPrA` / `run-qmJLSl` |
| extraction | 8/8、8/8 | 8/8、8/8 | `run-UYixNS` / `run-ry5Nj1` |
| decision | 4/4、4/4 | 4/4、4/4 | `run-BLob5P` / `run-3OsQBW` |
| mixed | 10/10、10/10 | 10/10、10/10 | `run-v3dTJX` / `run-LPql0T` |
| availability | 12/12、12/12 | 11/12、11/12 | `run-vrxa09` / `run-NdQWpT` |
| support | 12/12、12/12 | 10/12、10/12 | `run-t12EF9` / `run-FuAyYH` |

正式 gate v13 核对十六份当前 manifest，结果为失败，十次质量失败均保留。manifest 集合摘要为 `6cef935c77583b641627b3cce1f7e435e28689d47da293b7c892744354e7452d`。

| 失败题（均两轮） | 未通过的检查 | 相对 v17 |
| --- | --- | --- |
| injection、security_quote | expected_status、required_terms、expected_citations | 持续失败 |
| known_schedule_before_command | expected_status、required_terms、expected_citations | 新增退步 |
| another_entity_cannot_establish_availability | expected_status、expected_citations、forbidden_terms | 新增退步 |
| unrequested_absence_does_not_block_value | expected_status、required_terms、expected_citations | 新增退步 |

[正式比较器](ollama-answer-comparison.md)逐项核对两侧 142 题、568 项布尔检查。相对 v17，`forged_system` 两轮整题恢复，三道新失败题各两轮退步：整题改善/退步 **2/6**，布尔改善/退步 **4/18**。相对 v15 为整题 **8/6**、布尔 **24/22**；即使总分高于 v15 的 130/142，也不能隐藏新增失败或判为门槛通过。两次比较均正常完成并返回质量失败状态 2。

v18 部分报告不能参加完整双轮比较。其停止题 `silence_cannot_establish_availability` 在本次两轮均协议和质量通过，但这只说明本次未复现该协议拒绝，不证明支持判断正确或全局质量改善。

完整双轮共 426 次发送前许可：冷加载 1、复用 425、重配置 0，推理期间采样 2370 次；最低可用 8.64 GiB。单题中位耗时 8062.5 ms，v17 为 9156 ms；不同观察的耗时不能作为质量改善依据。

## 六题单次诊断

预定六题各执行一次，全部协议完成，四题质量通过、两题失败；独立于完整双轮，不补分。

| 题目 | 选择 / 核对证据数 | 证据集合一致 | 核对类型分歧数 | 质量结果 |
| --- | --- | --- | --- | --- |
| injection | 1 / 2 | 否 | 0 | 失败，最终弃权 |
| security_quote | 2 / 1 | 否 | 1 | 失败，最终弃权 |
| forged_system | 1 / 1 | 是 | 0 | 通过，value/stated |
| silence_cannot_establish_availability | 1 / 2 | 否 | 1 | 通过，最终弃权 |
| price_availability_undetermined | 1 / 1 | 是 | 1 | 通过，availability/unavailable |
| two_absences_establish_availability | 2 / 2 | 是 | 2 | 通过，两项 availability/unavailable |

沉默题的问题分类为一项 availability，核对仍标为 unavailable 并选择两个片段，局部支持计数为真；最终正确弃权来自独立证据集合不一致。schema 不再允许空证据支持，却未让核对正确区分资料沉默与明确未定。另三道新增回归未在这六道预定诊断中，完整报告没有模型正文或逐项语义数据，不能据此推断其内部原因。

诊断额外取得 18 次许可（冷加载 1、复用 17、重配置 0），最低可用 11.2 GiB。十六份完整报告、六份诊断及两份正式比较均保存在 `.local-model/` 下，目录/文件 0700/0600 已核对；执行索引、gate、比较索引和审计快照同为私有，未纳入 Git。冻结文件摘要再次核对未变，项目锁已解除，运行时未变，模型已按原策略自动释放。

## 工程验收与边界

Rust 1.96/1.99 各通过完整 fmt、Clippy `-D warnings` 及 351 项测试；286 项依赖显式服务/公网配置的用例默认忽略。问答及显存预算/观测 Node 共 53 项、前端 lint/typecheck/build 及 11 项单元测试通过。正式 gate 离线重算 v15/v16/v17 的历史报告，逐字段保持原 12/20/6 次失败；新增 profile 不改变旧请求或门槛。

隔离 `make smoke` 通过 281 项真实服务测试（含 175 项 PostgreSQL），生产镜像中 v14–v19 各 71 道清单、实际预览、双入口 HTTP/代理与重启持久化验收通过。测试项目 `personal-ai-smoke-8c90d175105bfc3f` 的专属容器、网络、数据卷均已清理并只读复核。

未运行本机浏览器/index/MinIO/公网、游戏并行及 vLLM。无数据库迁移、配置开关或页面改动。下一步须处理实体/属性支持、未被问及缺项与命令/事实区分，保留全部旧失败和新增回归；不能仅靠强制非空证据切换默认或开放私有执行。
