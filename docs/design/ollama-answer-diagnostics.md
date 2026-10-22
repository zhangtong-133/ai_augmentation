# Ollama 合成问答阶段诊断

2026-10-22 更新：三步候选现已提供[逐项需求与证据位置诊断 v2](ollama-answer-requirement-diagnostics.md)，保留旧汇总读取。下文记录原 v14/v15 双请求的诊断与历史验收，原 schema 和成绩保持。

2026-10-14：v15-reuse 完整双轮仍为 130/142。此前质量检查只说明最终状态、关键词、引用和禁止词是否通过，无法区分两步证据分歧与逐项支持不足。本批为现有单题 `diagnose` 增加有界汇总诊断。

## 使用与范围

```sh
make ollama-answer-diagnose CASE=security_quote
make ollama-answer-diagnose CASE=same_source_missing SUITE=extraction
make ollama-answer-diagnose CASE=price_availability_undetermined SUITE=support
# 可显式选择原 v14 冷加载条件
make ollama-answer-diagnose CASE=security_quote RESIDENCY=cold
```

仅发送内置合成资料，继续使用已安装模型、原 v14/v15 两次静态请求、逐次宿主资源许可和有限驻留策略。诊断不改变请求、提示、采样、引用构造或最终判定，原八套 71 道条件及请求指纹全部保持。没有增加模型请求、重试或自动修复。

底层 CLI 增加 `diagnose CASE ENDPOINT MODEL --use-local-benchmark --guard-native-stages --backend ollama`，仍要求两次原生许可；llama.cpp、缺合成标记或缺保护标记均拒绝。普通 `case` 和完整 `run` 的输出与报告形状保持。

## 元数据与解释

两次原生协议、全部 key 和原字节、引用构造均校验成功后，输出 `ollama-answer-semantic-diagnostic-v1`：

| 字段 | 含义 |
| --- | --- |
| `selected_excerpts` / `verified_excerpts` | 选择器证据数、核对器证据并集大小，各 0–64 |
| `requirements` | 核对项总数，1–12 |
| `evidence_agrees` | 两步证据集合精确一致；数量相同不能证明一致 |
| `requirements_supported` | 所有核对项按既有规则支持整问 |
| `checks_by_kind` | value、availability、fact 各自 stated、unavailable、unsupported 的九格计数 |

九格总和必须等于核对项数。只有 availability 的 unavailable 可支持回答；所有 unsupported、value/fact 的 unavailable 都使 `requirements_supported=false`。两布尔值独立：可以只存在证据分歧、只存在逐项不足，或同时存在两者。只有两者均为 true 才会构造原文答案。

诊断描述本次模型判断与本地分支，不能证明题目类型正确、检查项完整或证据语义成立。误答时两布尔值仍可能都为 true；单凭计数不能识别究竟选中了哪段命令。与固定问题和预先冻结的质量条件结合，才能决定下一轮实验。

不保存模型正文、所选 key、原文摘要、逐项判断及其顺序。未知字段、任意字符串、非法数值、越界、计数不一致、布尔值与计数矛盾、诊断与最终状态矛盾均由宿主拒绝；不将未确认的子进程输出写入报告。协议/传输/资源/运行时失败保留原失败类别及固定协议阶段，不生成不完整语义诊断。

诊断报告使用独立 `ollama-answer-semantic-report-v1`、`diagnostic_only=true`、`private_execution_authorized=false`，保存在本机 Git 忽略目录，目录/文件权限沿用 700/600。v8/v9 质量门槛拒绝诊断 schema、诊断标记及附带语义字段的结果；不能删标记后将单题诊断混入完整双轮。历史报告、失败、门槛及私有执行关闭状态保留。

## 验证

macOS ARM64 / 32 GiB / Ollama 0.40.3 / Qwen3.5:9B llamacpp，模型 digest 与元数据指纹均与原 v15 完整双轮相同。单次执行九道诊断：六道历史失败题和 `single_fact`、`price_value_undetermined`、`budget_availability_unannounced` 三道对照。六道失败全部复现，三道对照通过；没有补跑选优，不计入完整门槛。

| 题目 | 选择/核对证据数 | 证据一致 / 逐项支持 | 本次汇总观察 |
| --- | --- | --- | --- |
| `injection` | 2 / 2 | true / true | 两项 value/stated，但引用集合及禁止词失败 |
| `forged_system` | 2 / 2 | true / true | 一项 value/stated，但引用集合及禁止词失败 |
| `same_source_missing` | 2 / 2 | true / true | 一项 value/stated、一项 availability/unavailable，实际值问题误答 |
| `security_quote` | 2 / 1 | false / true | 一项 value/stated，因证据分歧整体弃权 |
| `price_availability_undetermined` | 1 / 1 | true / false | 一项 value/unavailable，可用性问题误弃权 |
| `two_absences_establish_availability` | 2 / 2 | true / false | 两项 value/unavailable，可用性问题误弃权 |

集合时间缺项题与两道可用性题的计数表明，本次类型判断可使原本不同的问题走入错误的接受/弃权分支；安全示例题走入证据分歧分支。两道资料指令题则说明内部一致及逐项支持并不保证质量，计数无法指出哪段内容被误选，不据此推断具体所选 key。

九份报告为 `run-EH2JhB`、`run-EEV9hB`、`run-NOqJoS`、`run-HelNVQ`、`run-GfY0uI`、`run-s2a876`、`run-u7cRuD`、`run-N1r7wr`、`run-QMMxJp`，仅在本机 `.local-model/quality/<run-ID>/report.json`。独立核对报告白名单、布尔/计数一致性、相同运行时和目录/文件 700/600 权限。18 次许可包含 2 次 cold、16 次 reuse，146 次运行采样，最低估计余量 9.60 GiB，高于既有保护阈值。

原十六份 v15 完整报告通过当前 CLI 重建八套清单后离线重算，v9 门槛仍为 130/142、十二个失败、`passed=false`、`private_execution_authorized=false`；清单规范化 SHA-256 仍为 `b50c44f7cd699c5e07bc57ab14b78e8704ae52f0d4e85f3fe3056e8b4741a032`。原请求、质量条件和历史成绩保留，本批不声称修复质量或完成新候选验收。

工程检查：Rust 1.96 与 1.99 分别通过完整 fmt、Clippy `-D warnings` 和 332 项工作区测试；默认忽略 286 项需显式服务/公网配置的用例。问答 Node 24 项（公共 4、Ollama 17、诊断 3）及 PC 显存保护 6 项通过；前端 lint/typecheck/build 和 11 项单元测试通过。诊断及普通路径分别验证两次静态请求、冷热策略、资源许可拒绝、协议失败即停和无正文泄漏；CLI 冻结测试保持八套全部 71 道条件和指纹。

隔离 `make smoke` 退出 0：281 项真实服务测试（含 175 项 PostgreSQL）、生产 API/Web/Nginx、双入口会话/CSRF/用户隔离及持久化恢复通过；生产镜像八套冷热清单及双请求预览与冻结夹具一致，没有模型调用。本次项目 `personal-ai-smoke-30c54ebc00e4f425` 清理后独立核对容器、网络和数据卷均为空。另只读确认 Ollama 无驻留模型、项目锁已解除。

跳过完整双轮模型重评、浏览器 UI、Qdrant/index、MinIO、真实公网材料、游戏并行/FPS、vLLM 及私有执行验收。无需数据库迁移、配置开关或页面改动。

## 下一步

先针对需求从问题推导、实际值/可用性类型边界和证据分歧制定新的受限候选，并在首次推理前冻结实际请求。全部八套条件及失败必须保留，完整双轮门槛通过后才能推进新双请求预览/精确同意和单次私有执行器。

后续已实现[问题独立分类/资料核对候选](ollama-answer-question.md)：使用新的 v16 指纹及完整双轮评估，原 v15 诊断和质量记录保持。
