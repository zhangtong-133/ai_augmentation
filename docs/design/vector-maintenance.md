# 残留向量显式清理

提供 `vector-maintenance gc` 运维命令，分页核对指定用户、集合和模型的向量引用。PostgreSQL 中没有相同用户与文档 ID 的点才是候选，包括删除用户后外键级联删除文档留下的向量。默认只预览，执行必须显式确认；不调用 Embedding、不读取文档正文或向量值，不执行迁移、不创建集合，也没有新增 HTTP 管理端点。

## 使用

配置 `DATABASE_URL`、`QDRANT_URL`、`QDRANT_COLLECTION`、`OPENAI_EMBEDDING_MODEL`、`EMBEDDING_DIMENSIONS`，可选 `QDRANT_API_KEY`。这里模型名只标识已有点，不需要 OpenAI 密钥。API 生产镜像包含该命令，本地也可用 `cargo run -p api-server --bin vector-maintenance -- …`。

```sh
vector-maintenance gc --user USER_UUID
vector-maintenance gc --user USER_UUID --after POINT_UUID
vector-maintenance gc --user USER_UUID --apply --exclusive-collection --writers-stopped
```

执行前须确认集合只属于当前 PostgreSQL 数据库，配置没有指向空库、错误环境或不完整的恢复库；暂停全部导入、索引及其他向量写入者，等待在途请求和后台工作结束。两个确认参数仅记录操作者确认，不会自动停服务或取得分布式锁。预览可以在线运行，但执行时必须重新核对上述条件。

每次至多扫描 100 个点，输出 JSON：`apply`、`scanned`、`eligible`、`changed` 和 `next_cursor`。不输出用户正文、文档来源或向量值。将非空 `next_cursor` 原样用于下一次 `--after`，沿用同一用户、集合和模型，直到返回 null；分页不是一致快照。执行前先整页核对数据库，逐点删除前再次检查文档存在性，成功删除须收到 Qdrant completed。

失败退出码为 1。失败前可能已有点删除，不提供跨 PostgreSQL/Qdrant 事务或回滚；不要把失败当成整页零写入，应从原游标重新运行，稳定点 ID 与范围过滤使重复删除安全。暂停写入仍是必要条件，再检查不能消除分布式竞争。

## 隔离与边界

Qdrant 使用用户与模型双重过滤，元数据投影只含用户、模型、逻辑 ID 和文档 ID；不获取 text/source/tags/vector。校验用户、模型、文档 UUID、稳定点 ID、重复点和游标推进；异常返回停止，不能把畸形数据当成可删除点。删除继续绑定用户、模型和逻辑点 ID。PostgreSQL 只使用 `documents(user_id,id)` 做 EXISTS 查询，数据库错误直接返回，绝不当作不存在。

本命令不会修改 PostgreSQL 记录，不删除有效文档的任何点，不处理仍存在文档的旧分块、失效模型或质量问题。所有旧模型/集合需分别显式运行；它不自动发现已删除用户，需要提供待核对用户 UUID。若写入恢复后出现迟到点，需要停止并排空写入后从头再扫描。

这是显式维护能力，不是用户删除事务的一部分或全局后台自动回收器。自动任务、跨环境恢复协调及自动发现删除用户仍是后续扩展；检索已有的 PostgreSQL 所有权核验继续生效。

滚动读取依据 [Qdrant Scroll points 官方协议](https://api.qdrant.tech/api-reference/points/scroll-points)，复用仓储稳定点 ID 和已有等待完成删除协议。

## 验证

`make smoke-index` 已接入真实 PostgreSQL/Qdrant/CLI 测试：102 个点跨页扫描、预览零修改、清除 101 个无文档引用、有效文档保留、重复执行、用户/模型隔离、删除用户后的残留清理，以及元数据故障时不删除。还会执行原有索引、语义检索、双入口权限和重启持久化验收。CLI 参数测试覆盖显式用户、错误游标、重复参数和执行确认边界。

2026-10-02 验收：Rust 1.99 `make check`、前端 lint/typecheck/build、完整 `make smoke-index` 通过，包含 135 项 PostgreSQL 测试及新增真实清理 CLI 闭环。Qdrant 两项集成测试通过，额外验证伪造点 ID 和非法游标拒绝；API 生产镜像中的命令 `--help` 已实测。隔离容器、网络及数据卷已清理。本轮无 UI 改动，未重跑 Playwright 或对象存储专项；模型使用本地夹具，未调用真实付费服务，未清理实际环境数据。
