# RSS 评分覆盖率与顺序扰动验收

## 分阶段计划

1. 在离线比较报告和质量门槛的套件比较中加入覆盖率，复核上一批 v4 真实报告。
2. 新增独立冻结的顺序扰动套件，明确内容充分条目必须给分；原三套语料、条件和业务 v4 请求不变。
3. 默认 4B 在同一模型/运行配置下完成两轮新套件，记录按语义条目对齐的换序差异和弃权。保留所有失败，不因未通过而放宽条件、改提示或反复重试。每阶段验收后提交推送。

## 阶段一：覆盖率

`local-value-compare` 输出新增 `coverage`，现有门槛报告的 `suites[].comparison` 同时包含它。统计单位是“条目 × 轮次”，不按检查条件重复计数；0 是已评分，null 是弃权。协议失败场景的全部条目计入 `protocol_failed`，后续场景计入 `not_run`，均不能填成 null 或 0。

分组仅解释冻结条件：`required_score` 是 minimum 或 prefer 的 higher；`expected_abstention` 是显式 abstain；其余为 `optional_score`。required 优先于 abstain，矛盾条件仍由原质量检查拒绝。这不是对真实材料是否充分的独立判断。每组和总计保留 expected/scored/abstained/protocol_failed/not_run，弃权率分母为已完成条目，完成率分母为预期条目；零分母输出 null。

上一批 v4 已保存双轮报告经离线复核：

| 套件 | 已完成/预期 | 已评分 | 弃权 | 可选评分组弃权 |
| --- | --- | --- | --- | --- |
| baseline | 20/20 | 12 | 8 | 4/10 |
| challenge | 26/26 | 16 | 10 | 6/10 |
| regression | 6/6 | 4 | 2 | 0/2 |

要求给分组均无弃权，显式要求弃权组均弃权。原上限条件允许 null，因此原合成门槛通过与可选组高弃权率可以同时成立。不得据此宣称真实用户材料质量达标。

验收：8 类比较器测试（含部分失败、未运行、零分、重复条件及零分母）、6 类门槛回归通过；真实旧报告未修改，未调用模型。全仓 Rust fmt/Clippy/tests 与前端 lint/typecheck/build 通过（HTTP 监听测试需在沙箱外运行）。本阶段未改数据库/API/页面，PostgreSQL 与浏览器专项未重跑。

## 范围与后续

本批材料仍是仓库固定合成输入，不读取用户数据库或私有订阅，也不将它们冒充真实新闻。后续真实材料验收需要来源、抽样规则和独立标注。私有检索问答仍需逐来源引用与精确分享授权。模型服务继续保留至少 6 GiB 显存监督；vLLM 与实际游戏压力测试独立保留。

## 补充阶段：消除权限测试并发冲突

上一批中间提交 `fabc722` 的 CI 在 `feed_operations` 两个测试并行 GRANT 时出现 PostgreSQL `XX000: tuple concurrently updated`。不同角色仍会修改相同系统目录行；DROP OWNED 同样可能与另一个测试的 GRANT 冲突。测试二进制内以 Tokio 互斥锁覆盖两个测试各自的完整角色创建、授权、读取验证和清理流程，业务查询与权限不变。

验收：使用仅绑定回环地址的一次性 PostgreSQL，保持 `--test-threads=2` 连续五轮运行两个权限测试，共 10 次全部通过；仍验证元数据可读、私有字段不可读及写入拒绝。Rust 格式检查通过；本阶段只有测试隔离修改，前端沿用阶段一结果。临时容器和卷已删除。

## 阶段二：冻结的六排列套件与独立门槛

新增 `SUITE=order`（`rss-order-v1`），三条合成材料的全排列共六个场景，每条材料恰好在首/中/尾位置各出现两次。这是三个独立条目、六种顺序，不能当作 18 条独立新闻。

所有标题都命中同一个 Rust 关键词、时间相同，以相同规则分数让订阅编号决定位置；通过正式业务规划器生成实际请求，不在发送时任意改排。测试核对模型实际收到的 JSON 顺序、六个不同请求指纹、相同正文与检查条件。材料取自恢复回归场景，但天气标题明确改为“Rust 之外的周末天气”，用于控制规则排序；原恢复回归文件未改。

相关条目 minimum 60；天气 minimum 0 与 ceiling 20 同时成立才通过，因此 null 不再满足该条条件；不足内容仍须 abstain。v4 对应 80/0/null，其他 profile 仍按原数值范围判断。六场景指纹在真实运行前冻结，不改业务提示、默认模型或原三套门槛。

新增 `local-value-order` 离线验收入口：至少两份完整套件报告、当前 Rust 生成的同 profile manifest、相同模型/运行指纹、独立不重叠时间段。按语义标签对齐评分，分别检查每轮换序是否一致及各轮是否重复一致。所有质量条件、轮内一致性、跨轮一致性均通过才退出 0；有效报告但条件未通过退出 2；输入无效退出 1。部分协议失败未观察到差异时，一致性为 null，不能说通过；已观察到差异则 false。报告只保存计数、标签、分数和指纹，不保存正文或模型原始输出。

```sh
make local-value-benchmark-preview SUITE=order CASE=order_1
make local-model-start
make local-value-benchmark SUITE=order
make local-value-benchmark SUITE=order
make local-model-stop
make local-value-order LEFT=第一轮报告.json RIGHT=第二轮报告.json
```

新门槛独立于原三套合成门槛；不得用旧门槛通过代替新门槛，也不自动启用业务。自动化加入 CI 与生产镜像 smoke 的离线 manifest 检查。阶段二工程验收包括 12 项领域质量测试、5 类换序报告测试，以及原基准/比较器/门槛回归。全仓 Rust fmt/Clippy/tests 通过；5 类基准、8 类比较器、6 类原门槛及 6 类显存/模型选择测试通过。原三套当前 manifest 与上一批真实报告逐字段一致，六个换序请求均为 1558 字节。前端沿用阶段一 lint/typecheck/build，本阶段无页面变更，不重跑浏览器专项；生产镜像和隔离 core smoke 通过，包含 170 项 PostgreSQL、41 项学习 HTTP、6 项评分 HTTP、6 项 Redis 及两种代理入口的核心验收；新套件已在打包后的 CLI 中验证，测试栈已清理。
