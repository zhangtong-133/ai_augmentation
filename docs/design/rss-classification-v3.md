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
