# Sprint 3：Redis 短期记忆适配器

## 已交付与边界

`crates/storage-redis` 实现现有 `MemoryStore::append/recent`，另提供幂等 `clear`。这是供可信服务端调用的存储层，不是聊天 API：尚未接入 API、Agent、模型上下文或浏览器，不改变现有问答行为，也不自动写入长期记忆。无数据库迁移及新增运行时环境变量。

`ConversationId` 指服务端管理的对话，不是登录 Cookie。键由用户与对话 ID 的独立 SHA-256 摘要组成，避免分隔符碰撞；每用户使用同一 Redis hash tag。调用者必须先认证用户并验证对话归属，不能直接采用客户端提交的用户 ID。摘要不是加密，正文仍是明文；Redis 必须使用私有网络及访问控制。

## 数据与限制

- 构造参数：TTL 1–86400 秒、每对话保留 1–100 条；每用户最多 32 个活跃对话，超额返回 `Conflict`，不淘汰别的对话。
- 单条 key 为 1–128 字节、value 为 1–4096 字节，不能全为空白或含 NUL；完整 JSON 最多 8 KiB。时间戳由可信调用方传入，仅供展示，不参与排序/过期。
- 追加按 Redis 执行顺序排列，裁剪最旧条目，并为整个窗口续期；`recent` 返回最新 N 条，内部按旧到新排列，N 必须为 1–配置容量，读取不续期。
- Lua 脚本用 Redis 时间清理过期名额，原子完成配额检查、追加、裁剪和续期。用户索引保留至最晚到期的对话；清空同时删除对话与配额成员。
- 单用户正文上限约 25 MiB（32 × 100 × 8 KiB），不含 Redis 开销；这不是实例全局内存上限。部署需独立命名空间、内存预算及 `noeviction`，避免索引被单独淘汰导致配额失真。禁止其他程序直接修改 `memory:v1:` 键。

连接和命令共用 2 秒超时，错误脱敏；依赖故障不能伪装成空列表。当前按操作建立连接，不自动重试。超时/取消不能撤销已经提交的写入，追加不幂等；调用方不能盲目重试。脚本不提供故障回滚，Redis 运维、备份和持久化策略不应把临时缓存当作可靠聊天历史。

## 验证与下一步

`make check` 运行参数/键隔离与超时测试。真实 Redis 测试需要一次性数据库：

```bash
TEST_REDIS_URL=redis://127.0.0.1:6379/ make test-redis
```

测试使用随机用户命名空间，不执行 `FLUSHDB`；覆盖用户/对话隔离、顺序/裁剪、重连、读不续期、写续期、并发配额和清空释放。CI 的 Rust job 提供 Redis 7 服务并运行此命令。

已交付[对话归属](sprint-3-conversations.md)和[持久化用户消息 API](sprint-3-messages.md)。消息采用独立的 `RedisMessageCache` 版本快照，不直接复用本文的非幂等 `MemoryStore::append`；本文容量/TTL 描述仅适用于原追加式适配器。Scheduler、模型自动记忆和 Agent 工具循环仍独立交付。

实现参考：[redis-rs 异步连接](https://docs.rs/redis/latest/redis/aio/struct.MultiplexedConnection.html)、[Redis EVAL](https://redis.io/docs/latest/commands/eval/)。

### 验收记录（2026-09-22，macOS / Docker）

`make check`、前端 lint/typecheck/build、Compose 配置和差异检查通过。一次性 Redis 7 的 3 项集成测试通过，包括不同 TTL 共存时的过期名额回收；无响应服务的 2 秒超时测试随工作区测试通过。本步未改 API/UI，未重跑 PostgreSQL/Qdrant/MinIO 专项、HTTP smoke 或浏览器验收；未调用模型。测试容器与临时数据已清理。
