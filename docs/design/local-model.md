# 独立本地模型与游戏显存预算

本批按三阶段验收、提交和推送：① 供应商无关本地推理端口和独立本地聊天 SSE 适配；② 精确地址/模型/分享内容授权、一次执行和现有私有页面观察；③ 项目私有部署、显存预算和真实模型兼容性验收。

## 推理边界

`llm::local::LocalTarget` 仅接受规范 HTTP loopback 地址与明确模型标签，不接收凭据、路径、代理或云模型标签。`llm-local` 独立实现 `/v1/chat/completions` 的受限 SSE profile，禁用代理/重定向/工具/thinking，设置 JSON、最多 2048 输出 token。部署机固定 8192 上下文、单模型/单并发。授权与推理后端分离，后续可换 vLLM；仍须单独验收真实协议和资源预算。完整提示最大 5632 UTF-8 字节，保守预留角色模板和输出空间；超限在发送前报错，不截断用户材料。适配器不保证其他服务兼容。

SSE 逐字节拆包，限制单行 64 KiB、传输 256 KiB、文本 24 KiB。绑定返回模型、单候选与 assistant 角色，仅 `finish_reason=stop` 且随后 `[DONE]` 可候选完成。必须等正常 HTTP EOF；截断、长度终止、thinking/工具、错误、终态后内容和非法 UTF-8 不返回成功。网络 60 秒限时，不重试推理；取消 future 关闭本地读取，已送请求不保证服务立即停止。临时正文不是已验证建议，仍需领域校验、来源复核与持久化。

## 显存约束

用户选择与游戏并行，至少预留 6 GiB。2026-10-04 本机实测 RTX 5070 Ti：16303 MiB 总显存、2309 MiB 已使用、驱动 591.86。默认从 Qwen3 4B Q4_K_M 开始（Qwen 官方 GGUF 文件约 2.5 GB），单模型、单并发、限制上下文；模型文件大小不等于实际显存。

部署须先核实 GPU，再测实际峰值。软件预算不是 GPU 硬分区：其他进程和游戏随时可增加占用。启动/发送前检查可用量，运行监控发现不足时停止本项目模型服务；无法查询显存则拒绝启动 GPU 推理。保留显式停止入口，不自动拉起或后台重发。

## 授权与执行

追加 0040 迁移，只增加本地地址和允许空订阅连接绑定；本地记录在 SQL 的连接 ID/版本为 NULL，私有 JSON 为兼容现有页面输出空字符串/0，加 `local_endpoint` 区分。摘要使用独立 learning-local-consent-v1 域绑定 owner、请求、来源、地址、模型、材料摘要和期限。草稿与订阅共用 5 分钟期限及额度，不保留证据副本；提示预算超限拒绝草稿，不截断材料。

默认 `LEARNING_LOCAL_ENABLED=false`。私有会话/CSRF 下的 `local-model-authorizations`、`approve-local` 分别创建草稿和确认分享/本机计算。订阅批准入口拒绝本地请求；两类领取入口互不领取另一类。来源、材料、自评、技能和计划撤销继续使本地授权/建议失效。

`local-review run OWNER REQUEST ENDPOINT MODEL --use-local` 从数据库重建已批准的原请求，先比对地址/模型，再一次领取。发送前原子保存发送标记，再调用独立适配器；取消、来源改变、超时、崩溃、严格返回失败都不重新领取或发送。Redis 私有临时正文和已保存状态观察沿用现有端口；网页仅保存授权与观察，执行仍须本机明确命令。`local-review show OWNER REQUEST` 查询原状态。建议仍须用户独立核验，不修改自评。

运维审计增加 `execution_kind=local/subscription`，从已授权的连接元数据判断类型，不读取本地地址、模型、摘要或正文，也不扩大最小列权限。

参考：[llama.cpp 官方 server](https://github.com/ggml-org/llama.cpp/tree/master/tools/server)、[Qwen 官方 GGUF](https://huggingface.co/Qwen/Qwen3-4B-GGUF)、[vLLM 资源配置](https://docs.vllm.ai/en/latest/configuration/conserving_memory/)。真实调用和质量验收独立记录，夹具不替代真实能力评估。
