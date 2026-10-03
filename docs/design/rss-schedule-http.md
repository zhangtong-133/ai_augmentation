# 周期 RSS 用户授权 HTTP 接口

周期采集现在通过登录会话提供不可变预览、精确同意、取消、分页历史和审计。复用已有授权仓储及后台执行器，不新增迁移或部署配置。用户配置页面仍待实现。

## 接口

所有路径均以 `/api` 开头，所有响应（包括错误）均为 `Cache-Control: no-store`。写操作要求现有 CSRF/同源保护；所有查询使用会话用户，客户端不能指定 owner。

| 方法与路径 | 请求与结果 |
| --- | --- |
| POST `/feed-subscriptions/{id}/schedules` | `schedule_id` UUID、`starts_at_unix_ms` / `ends_at_unix_ms` 十进制字符串、`interval_hours` 数字；返回 201 和已保存草稿 |
| GET `/feed-schedules?after={id}` | 当前用户历史，最多 20 条，返回 `items` 和 `next_cursor` |
| GET `/feed-schedules/{id}` | 计划、摘要、状态与批准时间 |
| GET `/feed-schedules/{id}/audit` | 当前用户计划的状态事件和时间 |
| POST `/feed-schedules/{id}/approve` | `accepted_digest` 和 `acknowledge_recurring_source_requests: true` |
| POST `/feed-schedules/{id}/cancel` | 严格空对象 `{}`；重复取消幂等 |

所有 JSON 和分页查询拒绝未知字段。输入时间必须是正数、无前导零、在 i64 范围内的十进制字符串；输出的时间及订阅版本均使用字符串，避免 JavaScript 精度损失。非法输入为 400，JSON 类型/未知字段按 Axum 拒绝为 422；跨用户及不存在资源为 404，过期、失效或错误摘要同意为 409。

## 同意与运行边界

草稿包含精确源 URL、订阅版本、开始/结束时间、间隔、最大次数及固定请求策略。开始至少在预览后 5 分钟，结束不超过预览后 7 天，间隔仅 1/6/24 小时，批准窗口 5 分钟。相同 ID 和输入重放返回原计划，不延长窗口。提交摘要必须匹配服务端保存的整个计划；必须明确同意周期源请求。只提交布尔同意、客户端自带计划或一次性采集同意均不能授权。

批准接口只保存授权，不领取、不派发、不访问 RSS 网络。`active` 表示已授权，不表示 worker 在线。后台运行仍要求 `SCHEDULER_MODE=local` 和 `RSS_SCHEDULES_ENABLED=true`；API 的 `RSS_COLLECTION_MODE` 仅控制手动一次性执行。部署关闭采集时仍可管理、授权或取消计划；客户端不可由 `/feeds/config` 推断 scheduler 状态。

每个用户每个订阅最多一个 active 计划。批准重放保持首次批准时间；取消后不能重新批准，需创建新计划。修改、停用或删除订阅会事务性作废授权；历史保留到用户删除账户。到期与取消后的执行/结果边界由[授权仓储](rss-schedule-store.md)和[执行器](rss-schedule-execution.md)复核。

Next.js 代理仅允许上述用户接口，内部跨用户扫描、领取、派发没有 HTTP 入口。

## 验证

真实 PostgreSQL HTTP 测试覆盖登录与 CSRF、严格输入、精确字符串时间、跨用户详情/批准/取消/审计隔离、错误摘要和未勾选同意、批准/取消重放、历史分页、无缓存及零传输调用。核心 smoke 通过 Next.js 和反向代理两个入口检查创建、授权、历史、订阅变更自动取消和撤销后拒绝重授权，包括手动采集关闭时的独立管理行为。

本轮同时修复上一提交 CI 的 scheduler 进程测试竞争：两个测试启动的真实进程会扫描共享数据库全部用户，因此在同一测试二进制内使用异步互斥保护完整数据库夹具生命周期，防止相互领取或跳过锁定任务。

2026-10-03 验收：Rust 1.99 `make check`、前端 lint/typecheck/build 和完整 `make smoke` 通过，包含 146 项 PostgreSQL 回归、8 项 RSS HTTP 测试、双网关及重启恢复。scheduler 隔离修复通过 Clippy，并在一次性数据库上连续运行 5 次进程测试。smoke 已清理测试容器、网络和数据。未重跑 Playwright、对象存储/向量专项，也未请求实际 RSS 服务。
