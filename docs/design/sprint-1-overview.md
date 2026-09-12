# Sprint 1：服务状态与今日概览

本轮补齐路线图中的 Dashboard 概览，仅展示已实现知识库的真实统计，不生成模拟新闻、学习任务或 RAG 数据。

## 接口与统计口径

`GET /api/overview` 必须使用有效用户会话；管理 Bearer Token 不代替用户身份。返回 `Cache-Control: no-store`，无登录/过期会话返回 401，存储故障返回脱敏的 503。

响应示例：

```json
{
  "timezone": "UTC",
  "day_start_unix_ms": 0,
  "day_end_unix_ms": 86400000,
  "generated_at_unix_ms": 1000,
  "knowledge": { "total_documents": 0, "total_chunks": 0, "imported_today": 0 }
}
```

服务端决定当前时间和 UTC 日界线，时间范围为左闭右开 `[start, end)`。UTC 今日对应北京时间当日 08:00 至次日 08:00；界面明确注明，不隐含浏览器本地日历日。后续用户时区配置需要单独设计。

文档总数与文本块总数覆盖当前用户全部文档，不受列表 20 条分页限制；今日导入量仅统计范围内创建的文档。重复导入失败不会增加数量。空库返回零，存储故障绝不转换为零。

## 架构与存储

在 `DocumentStore` 增加显式要求 owner 和时间范围的聚合端口；PostgreSQL 使用带 `user_id` 条件的单条聚合 SQL，统一快照读取计数，不向 API 传输原文或 chunks。无需新增迁移、依赖或配置。当前总量统计会扫描用户文档；大型知识库的计数缓存/增量汇总留待测量后优化。

## 前端行为

Next.js 代理增加只读 overview 白名单。服务状态分别检查 `/api/healthz`（进程存活）和 `/api/readyz`（数据库就绪），不代表 Redis、MinIO、Qdrant 或 LLM 已就绪；网络故障与非成功响应明确显示不可用，可手动重试。

登录后加载概览，导入成功自动刷新，也可手动刷新；无定时轮询，跨日后需刷新。退出卸载概览，用户切换按 ID 重建，取消过期请求。加载、空库和错误状态分开呈现，刷新失败不显示旧数字冒充最新值。信息流及学习模块仍标注未启用。

## 验证

Rust 路由测试覆盖会话要求、管理员 Token 不可替代、空库、用户隔离、导入后计数、no-store 与存储失败脱敏；单元测试覆盖 UTC 午夜切换。PostgreSQL 集成测试增加持久化聚合、空库、隔离和起止边界断言，通过 `TEST_DATABASE_URL=… make test-postgres` 在可丢弃数据库执行。无真实数据库时必须报告跳过。

前端验证执行 lint、typecheck、build；这些检查不能替代真实登录、导入及退出的浏览器联调。Compose smoke test 仍属未完成项。

2026-09-07 本地结果：`make check` 通过（14 个测试通过，2 个 PostgreSQL 测试跳过）；前端 lint/typecheck/build、Compose 配置校验通过。环境检查仍报告 Docker daemon 不可达，未执行真实 PostgreSQL、Compose smoke test 或浏览器联调。

后续状态：2026-09-12 已通过真实 PostgreSQL 和核心 Compose HTTP smoke test（含概览及重启持久化），详见 [验收记录](sprint-1-acceptance.md)。上段是历史记录；浏览器交互验证仍待完成。
