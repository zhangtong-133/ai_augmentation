# Sprint 4：受限 RSS 2.0 解析与去重

## 交付与接收范围

personal-ai-feeds::parser 提供离线 parse_rss、条目内容摘要、完整作用域键和 classify。读取调用方提供的字节，不获取订阅或文章，不写数据库，不调用模型。解析成功也不是采集授权；[不可变预览与精确同意](sprint-4-rss-collection.md)仍须由后续事务执行器领取。

本版是 RSS 2.0 的明确受限子集，不宣称完整 RSS/XML 兼容。只接收 UTF-8（可带 BOM）、XML 1.0、单个无命名空间的 rss version="2.0" 根及单个 channel。channel 要求非空纯文本 title、description 和符合现有 normalize_source 规则的 HTTPS link；item 必须有转换后非空的 title 或 description。被读取的字段不得重复或包含 XML 子元素，HTML 须通过转义文本或 CDATA 提供。未知普通元素可忽略，但其 XML 结构、属性、实体与资源上限仍会被校验。

命名空间声明、带前缀名称、Atom、RSS 1.0/RDF、DTD（含内部实体）、处理指令及未知实体整体拒绝。当前不支持 content:encoded、dc:date 等扩展，包含此类声明的常见 RSS 也会被拒绝；支持扩展应作为独立兼容性迭代，通过命名空间解析实现，不能直接丢弃前缀后识别 RSS 字段。元素和属性名称只接受 ASCII XML 名称子集。

quick-xml 提供事件读取、结束标签匹配和注释检查；模块额外校验 UTF-8/XML 合法字符、声明属性、属性值、根结构和引用。中间树有明确边界，并非无界 DOM：

| 限制 | 数值 |
| --- | --- |
| 整个输入 | 1 MiB，先检查实际字节长度 |
| XML 层级 | 32（含 rss 根） |
| XML 元素总数 | 20,000，包含忽略元素 |
| 单元素属性数 | 64，限制重复属性检查的工作量 |
| 原始 item 数量 | 100，重复项也计数 |
| channel/item 标题 | 512 Unicode 字符 |
| channel 描述/item 摘要 | 8192 Unicode 字符 |
| 非空 GUID | 2048 UTF-8 字节 |
| 链接 | 复用 2048 字节及 HTTPS/443/DNS 域名约束 |
| pubDate | 256 Unicode 字符，仅保留来源元数据 |

HTML 通过 scraper 的离线片段解析转换为纯文本；字段长度在转换前后都检查，不截断。忽略 script、style、template、noscript、iframe、object、embed、svg、math、head 子树；保留普通正文，合并空白并在常见块元素边界分隔。文本遍历深度最多 32、访问次数最多 20,000。不会执行脚本、解析 CSS 或加载图片/enclosure/其他远程资源。输出仍是不可信文本，后续 UI 应作为文本渲染。

任何结构、字段、上限或重复冲突错误都会拒绝整个批次，没有部分成功输出。错误使用固定枚举，不包含 URL、GUID、XML 或正文；Feed/Entry 不自动派生 Debug。

## 身份、批次去重与内容更新

优先使用非空 GUID，保持解码后的不透明内容（包括非空 GUID 两端空白），不根据 isPermaLink 解释或访问它。没有可用 GUID 时，使用规范化 HTTPS link；只有空白的 GUID 视为缺失。若提供了非法链接，即使有 GUID 也拒绝此条目，避免把非法导航目标带入后续展示。链接移除 fragment，保留查询参数，不做网络请求或 DNS 信任判断。

身份键格式为 guid:<SHA-256> 或 link:<SHA-256>。摘要使用版本域、身份类型及原值的长度前缀编码，避免字符串拼接歧义。正文摘要独立覆盖标题、摘要、规范化链接和来源日期，不包含身份，因此同一身份的正文变化可以更新原记录。

同批次相同身份、相同正文摘要只保留第一次出现；相同身份、不同正文摘要返回 ConflictingDuplicate，整个批次失败，不根据来源顺序任意挑选内容。不同身份即使正文相同也保留，不实现跨来源正文相似度合并。

scoped_key 要求有效且非空的用户/订阅 UUID，返回 (user_id, subscription_id, entry_key)。classify 在完整键下比较已保存摘要，返回 New、Unchanged 或 Updated。其他用户或订阅的同身份记录不会影响结果。现有记录必须来自可信 owner-scoped 仓储；调用方提供的映射不是权限证明。未来数据库仍须复合唯一约束、所有者查询与事务 upsert，以处理并发；纯函数无法代替这些约束。摘要不是授权签名。

## 验证与下一步

本地夹具覆盖正常 UTF-8、BOM、XML 实体/CDATA、HTML 活跃内容移除和块边界、GUID 不透明性、规范化链接、账户/订阅隔离、内容更新、重复/冲突条目，以及编码、DTD/PI、命名空间、无效字符、畸形 XML、重复字段和各类超限拒绝。

验证使用 make check，以及前端 lint、typecheck 和 build。没有 API、存储、迁移或 UI 改动，本步不运行需要数据库/容器的 PostgreSQL、Redis 等外部服务集成测试、Compose smoke 或浏览器验收，也不发送真实订阅请求。

[订阅仓储、一次性采集授权与审计事务](sprint-4-rss-store.md)已实现；下一步实现 DNS/连接固定/超时/字节限制完整的传输适配器，再接入 HTTP 管理与页面。自动轮询、价值评分和 Daily Brief 尚未实现。
