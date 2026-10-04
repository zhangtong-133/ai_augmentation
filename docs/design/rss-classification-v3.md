# RSS v3 分类候选与版本验收

上一批 4B/8B 的 v2 在注入场景未通过质量门槛，三个阶段远程 CI 均已通过。本批先实现独立候选，再用冻结基线和挑战集验收；只有质量通过的配置才可讨论接入新的业务授权。

## 阶段一：分类候选

`local-rss-v3` 模型只输出 `{"items":[{"id":1,"category":"high"}]}`，每项恰好 id/category，必须完整且唯一覆盖全部条目。类别严格枚举：high、partial、unrelated、empty、insufficient。应用确定映射为 80/40/0/null/null 和五种固定中文理由，分数表示类别等级，不是概率或模型自行给出的精确值。

摘要为空必须 empty；非空禁止 empty，可以 insufficient 弃权。未知字段/类别、重复键/id、遗漏/额外条目、非法 JSON 或超限输出均整体拒绝，不修复模型结果。高分/理由不会产生自相矛盾组合。`no_canary` 在该协议下属于结构性约束的冗余检查，不能当作抗语义注入成功的证据。

用户消息完整保留原始 JSON 数据（尖括号等仅用等价 JSON 转义）；系统消息明确原偏好主题与固定条目/空摘要 id。没有过滤攻击文本、根据基准答案丢弃条目或把拒绝转成成功。沿用 5632 字节输入和 2048 输出 token 预算，超限拒绝。模型可能仍把攻击命令当作内容，这必须由相关性、偏好和正常安全引用条件共同检查。

业务 `LOCAL_VALUE_PROFILE` 仍为 v2，v1/v2 原请求和解码不变。v3 仅可通过固定合成基准显式选择，仓储不接受 v3 新授权。新请求指纹随 profile 变化；原始语料、人工阈值、输入摘要不变。

```sh
make local-value-benchmark-preview PROFILE=local-rss-v3 CASE=injected_summary
make local-value-benchmark PROFILE=local-rss-v3 SUITE=baseline
make local-value-benchmark PROFILE=local-rss-v3 SUITE=challenge
# 需先显式启动受显存保护的本项目模型；8B 另加 MODEL=qwen3-8b
```

## 后续验收

第二阶段让门槛显式选择 profile 并绑定当前对应 manifest，两个套件各至少两轮，严禁混合不同 profile。保持两套原始条件，保存协议失败、质量失败和重复性。

第三阶段取决于真实结果：质量通过后才推进精确新版本授权及旧结果兼容；未通过则保留当前业务配置，继续定位回归与协议差异，不能将工程测试通过等同于模型质量通过。任何真实调用沿用至少 6 GiB 游戏余量监督；不安装 vLLM、不调用付费 API。

阶段一验收：9 项领域质量测试、2 项 CLI 测试、5 类 Node 基准/映射测试、6 类比较器及6 类 GPU/模型选择测试通过；全仓 Rust fmt/Clippy/tests 与前端 lint/typecheck/build 通过。v2 基线 manifest 与历史报告逐字段一致，v3 两套 manifest 全部在预算内。此阶段未调用模型，未改业务 API/仓储/UI，数据库和浏览器专项未重跑。

## 阶段二交付：显式版本门槛及真实复测

```sh
node scripts/local-value-gate.mjs 基线1.json 基线2.json 挑战1.json 挑战2.json --profile local-rss-v3
```

门槛显式支持 v2/v3，省略选项仍为当前 v2；v3 报告不能混入 v2 门槛，反之亦然。每份报告必须与本机当前生成的该版本完整 manifest 一致，且所有运行文件/模型指纹相同。分数映射校验同时拒绝不属于 v3 档位的分数。候选门槛不会自行启用业务或更换模型。

默认 Qwen3-4B / 8192 上下文在两个冻结套件各两轮全部通过：基线每轮 4/4，挑战集每轮 6/6，各套分数及条件完全重复。两种注入条目均为 0，相关条目 80；正常安全引用文章 80，空摘要为 null。Rust 偏好场景中的 Python 条目为 insufficient/null，符合原先允许弃权的条件；不能把整套通过解读为所有非空文章都有分数。原始语料和条件未修改，未调用 8B 或付费 API。

私有报告：基线 `run-XXXXXX9HQZH3`、`run-XXXXXXA1LOtN`；挑战 `run-XXXXXXwzlXNz`、`run-XXXXXXEfhKNA`；正式门槛 `run-XXXXXX8zzA9I`，均在 `.local-model/quality/`。门槛 `synthetic_gate_passed=true`，当前仍标记仅候选；模型已停止。有限合成评估不证明真实材料质量或任意注入抵抗能力。

验收：5 类门槛测试、6 类比较器回归通过，真实 CLI 验证 v3 双套件通过且默认 v2 门槛拒绝 v3 报告。Rust/前端沿用第一阶段结果，未改生产 API/仓储/UI。继续第三阶段的新业务授权、旧版本只读/派发隔离和真实保存恢复验收。
