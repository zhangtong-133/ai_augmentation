.DEFAULT_GOAL := help

.PHONY: help env-check check fmt test test-postgres smoke browser-install browser-test web-install web-dev compose-config infra-up infra-down stack-up stack-down

help:
	@awk 'BEGIN {FS = ":.*## "} /^[a-zA-Z_-]+:.*## / {printf "%-18s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

env-check: ## 检查本地必需及可选工具
	@./scripts/check-env.sh

check: ## 执行 Rust 工作区格式检查、静态检查与测试
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace

fmt: ## 格式化 Rust 源码
	cargo fmt --all

test: ## 运行 Rust 工作区测试
	cargo test --workspace

test-postgres: ## 使用 TEST_DATABASE_URL 运行 PostgreSQL 集成测试
	cargo test -p personal-ai-storage-postgres --test postgres -- --ignored

.PHONY: test-tools
test-tools: ## 使用一次性 TEST_DATABASE_URL 验证工具执行、次数预算与审计
	cargo test -p personal-ai-tools
	cargo test -p personal-ai-agent-core tool_execution
	cargo test -p api-server --lib tool
	cargo test -p personal-ai-storage-postgres --test postgres tool_calls -- --ignored

.PHONY: test-agent
test-agent: ## 使用一次性 TEST_DATABASE_URL 验证受限计划、授权与执行
	cargo test -p personal-ai-agent-core knowledge_plan
	cargo test -p personal-ai-storage-postgres --test postgres agent_plans -- --ignored
	cargo test -p api-server --lib agent_plans -- --ignored

.PHONY: test-model-agent
test-model-agent: ## 使用一次性 TEST_DATABASE_URL 验证模型规划、检索/回答仓储、事务预算及一次性执行器
	cargo test -p personal-ai-agent-core model_
	cargo test -p personal-ai-storage-postgres --test postgres model_planning -- --ignored

.PHONY: test-replies
test-replies: ## 使用一次性 TEST_DATABASE_URL 验证回复执行器、金额 HTTP 与运维命令
	cargo test -p api-server --test replies -- --ignored
	cargo test -p api-server --lib paid_tests -- --ignored
	# 运维角色夹具修改共享表权限；串行运行，避免 GRANT/DROP OWNED 的 ACL 元组竞争。
	cargo test -p api-server --test reply_operations -- --ignored --test-threads=1
	cargo test -p api-server --test model_operations -- --ignored --test-threads=1
	cargo test -p api-server --test scheduler_operations -- --ignored --test-threads=1
	cargo test -p api-server --test mcp_operations -- --ignored --test-threads=1

.PHONY: test-redis
test-redis: ## 使用一次性 TEST_REDIS_URL 验证短期记忆隔离、配额与 TTL
	cargo test -p personal-ai-storage-redis --test redis -- --ignored

smoke: ## 构建隔离 Compose 环境，验证持久化与 HTTP 流程，并清理测试数据
	node scripts/smoke.mjs

browser-install: ## 安装锁定的 Playwright 依赖与当前平台的无头 Chromium
	npm --prefix tests/browser ci
	npm --prefix tests/browser run install-browser

browser-test: ## 在隔离 Compose 环境中验收；可用环境变量 BROWSER_SPEC 指定单个浏览器用例文件
	node scripts/smoke.mjs --browser

.PHONY: browser-test-public
browser-test-public: ## 额外验证真实公网网页导入（需要直连互联网）
	node scripts/smoke.mjs --browser --public-web

web-install: ## 安装锁定的前端依赖
	npm --prefix apps/web ci

web-dev: ## 启动 Next.js 开发服务器
	npm --prefix apps/web run dev

compose-config: ## 校验 Compose 配置，不启动服务
	@./scripts/compose.sh --env-file .env.example config --quiet

infra-up: ## 启动 PostgreSQL、Redis、Qdrant 和 MinIO
	@./scripts/compose.sh up -d postgres redis qdrant minio minio-init

infra-down: ## 停止本地服务
	@./scripts/compose.sh down

stack-up: ## 构建并启动完整本地服务栈
	@./scripts/compose.sh up -d --build

stack-down: ## 停止完整本地服务栈
	@./scripts/compose.sh down

.PHONY: smoke-objects
smoke-objects: ## 在隔离 MinIO/PostgreSQL 中验证原文存储及 HTTP 流程
	node scripts/smoke.mjs --objects

.PHONY: smoke-index
smoke-index: ## 验证隔离 Qdrant、Embedding HTTP 夹具和双入口文档索引
	node scripts/smoke.mjs --index

.PHONY: browser-test-index
browser-test-index: ## 使用本地模型夹具验证索引任务及双入口无头 UI
	node scripts/smoke.mjs --index --browser

.PHONY: test-feeds
test-feeds: ## 使用一次性 TEST_DATABASE_URL 验证 RSS 会话、授权及 HTTP 隔离
	cargo test -p api-server --lib feed_tests -- --ignored

.PHONY: test-feed-operations
test-feed-operations: ## 使用一次性 TEST_DATABASE_URL 验证 RSS 只读命令及权限
	cargo test -p api-server --test feed_operations -- --ignored

.PHONY: test-learning
test-learning: ## 使用一次性 TEST_DATABASE_URL/TEST_REDIS_URL 验证学习 HTTP、临时正文与结果隔离
	cargo test -p api-server --lib learning_tests -- --ignored

.PHONY: test-learning-operations
test-learning-operations: ## 使用一次性 TEST_DATABASE_URL 验证学习只读诊断及权限
	cargo test -p api-server --test learning_operations -- --ignored

.PHONY: test-subscription-connections
test-subscription-connections: ## 使用一次性 TEST_DATABASE_URL 验证订阅连接管理 HTTP 与用户隔离
	cargo test -p api-server --lib subscription_connection_tests -- --ignored

.PHONY: learning-acceptance
learning-acceptance: ## 在隔离环境中验收学习数据库、运维、HTTP 及双入口页面，不调用真实模型
	$(MAKE) browser-test BROWSER_SPEC=learning.spec.mjs

.PHONY: local-model-install local-model-start local-model-stop local-model-status local-model-probe local-model-test local-review
local-model-install: ## 在项目私有目录安装官方运行时/默认模型；MODEL=qwen3-8b 仅装候选
	python3 scripts/install-local-model.py $(if $(MODEL),--model "$(MODEL)",)

local-model-start: ## 前台启动单模型服务（可选 MODEL=qwen3-8b），监督至少 6 GiB 游戏显存
	node scripts/local-model.mjs start $(if $(MODEL),--model "$(MODEL)",)

local-model-stop: ## 停止本项目模型及其 GPU 资源，不停止其他程序
	node scripts/local-model.mjs stop

local-model-status: ## 核对 GPU 余量及本项目模型状态
	node scripts/local-model.mjs status

local-model-probe: ## 用固定测试材料验收真实模型协议，不读取用户记录
	node scripts/local-model.mjs probe

local-model-test: ## 验收显存预算、监控故障与停止边界
	node scripts/test-local-model-budget.mjs
	python3 scripts/test-local-model-install.py

local-review: ## 显式执行已批准的原本地核验请求（OWNER/REQUEST）
	node scripts/local-model.mjs review "$(OWNER)" "$(REQUEST)"

.PHONY: local-value
local-value: ## 显式执行已批准的本地 RSS 评分（OWNER/REQUEST），复用游戏显存保护
	@test -n "$(OWNER)" -a -n "$(REQUEST)" || (echo '需要 OWNER=用户UUID REQUEST=原授权UUID'; exit 1)
	node scripts/local-model.mjs value "$(OWNER)" "$(REQUEST)"

.PHONY: local-value-benchmark local-value-benchmark-preview local-value-quality-test
local-value-benchmark: ## 显式运行合成 RSS 质量基准（SUITE=baseline/challenge/regression/order/public_calibration/public_holdout）；先启动受显存保护的项目模型
	node scripts/local-value-benchmark.mjs run $(if $(PROFILE),--profile "$(PROFILE)",) $(if $(SUITE),--suite "$(SUITE)",) $(if $(MODEL),--model "$(MODEL)",)

local-value-benchmark-preview: ## 离线预览固定基准的精确分享提示及条件（CASE）
	node scripts/local-value-benchmark.mjs preview "$(CASE)" $(if $(PROFILE),--profile "$(PROFILE)",) $(if $(SUITE),--suite "$(SUITE)",) $(if $(MODEL),--model "$(MODEL)",)

local-value-quality-test: ## 验收合成质量判定、报告绑定与协议故障；不启动 GPU 或调用模型
	cargo test -p personal-ai-agent-core feed_value_quality
	cargo test -p api-server --bin local-value-benchmark
	node scripts/test-local-value-benchmark.mjs

.PHONY: local-value-compare local-value-compare-test
local-value-compare: ## 离线比较两份同条件质量报告（LEFT、RIGHT），不调用模型
	node scripts/local-value-compare.mjs "$(LEFT)" "$(RIGHT)"

local-value-compare-test: ## 验收报告条件匹配、部分失败统计及有界私有读取
	node scripts/test-local-value-compare.mjs

.PHONY: local-gpu-observe local-gpu-observe-test
DURATION ?= 30
SCENARIO ?= unspecified
local-gpu-observe: ## 只读采样 GPU 并保存私有报告（DURATION 秒、SCENARIO 场景标签）
	node scripts/local-gpu-observe.mjs --seconds "$(DURATION)" --scenario "$(SCENARIO)"

local-gpu-observe-test: ## 验收 GPU 观测边界、故障和取消，不使用真实 GPU
	node scripts/test-local-gpu-observe.mjs

.PHONY: recovery-test
recovery-test: ## 验收备份清单、迁移及文件完整性边界
	node scripts/test-recovery.mjs

.PHONY: deployment-check
deployment-check: ## 显式检查部署数据库迁移及恢复隔离状态，仅输出计数
	@test -n "$(CONTAINER)" -a -n "$(DATABASE)" -a -n "$(PROFILE)" || (echo '需要 CONTAINER=完整ID DATABASE=数据库名 PROFILE=current或recovery'; exit 1)
	node scripts/deployment-check.mjs --container "$(CONTAINER)" --database "$(DATABASE)" --profile "$(PROFILE)"

.PHONY: recovery-acceptance
recovery-acceptance: ## 在一次性 PostgreSQL 中验收真实备份及恢复边界
	cargo build --locked -p api-server --bin api-server --bin local-review
	node scripts/recovery-acceptance.mjs

.PHONY: recovery-acceptance-local
recovery-acceptance-local: ## 用合成证据验收受显存保护的真实本地模型、建议保存及恢复后不重发
	cargo build --locked -p api-server --bin api-server --bin local-review
	node scripts/recovery-acceptance.mjs --local-model

.PHONY: recovery-acceptance-objects
recovery-acceptance-objects: ## 在一次性 PostgreSQL/MinIO 中验收数据库与外部原文完整恢复
	cargo build --locked -p api-server --bin api-server --bin local-review --bin original-archive
	node scripts/recovery-acceptance.mjs --objects

.PHONY: recovery-acceptance-rss-local
recovery-acceptance-rss-local: ## 用合成 RSS 验收真实本地评分及恢复后不重发，需要显式启动保护服务
	cargo build --locked -p api-server --bin api-server --bin local-review --bin local-value
	node scripts/recovery-acceptance.mjs --local-rss

.PHONY: local-value-gate-test
local-value-gate-test: ## 离线验收按版本区分的多套件质量门槛，不调用模型
	node scripts/test-local-value-gate.mjs

.PHONY: local-value-order local-value-order-test
local-value-order: ## 离线核对两轮换序报告（LEFT、RIGHT）的评分覆盖及一致性
	node scripts/local-value-order.mjs "$(LEFT)" "$(RIGHT)"

local-value-order-test: ## 验收换序差异、弃权、失败与当前语料绑定
	node scripts/test-local-value-order.mjs

.PHONY: local-value-public-test
local-value-public-test: ## 验收公开文档改写双套件门槛及逐主题偏差统计
	node scripts/test-local-value-public.mjs

.PHONY: local-answer-benchmark local-answer-benchmark-preview local-answer-quality-test
local-answer-benchmark: ## 显式运行固定合成本地问答评估；先启动受至少 6 GiB 显存保护的模型
	node scripts/local-answer-benchmark.mjs run $(if $(MODEL),--model "$(MODEL)",)

local-answer-benchmark-preview: ## 离线预览问答合成材料、实际请求和预先冻结的条件（CASE）
	node scripts/local-answer-benchmark.mjs preview "$(CASE)" $(if $(MODEL),--model "$(MODEL)",)

local-answer-quality-test: ## 验收问答条件、固定 CLI 与私有报告边界，不启动 GPU
	cargo test -p personal-ai-knowledge answer_quality
	cargo test -p personal-ai-llm-local
	cargo test -p api-server --bin local-answer-benchmark
	node scripts/test-local-answer-benchmark.mjs
	node scripts/test-ollama-answer-benchmark.mjs

.PHONY: ollama-answer-benchmark ollama-answer-benchmark-preview ollama-answer-diagnose ollama-answer-quality-gate
ollama-answer-benchmark: ## 在 macOS ARM64 显式评估已安装 Ollama 模型；SUITE=challenge/coverage 选独立语料
	node scripts/ollama-answer-benchmark.mjs run $(if $(MODEL),--model "$(MODEL)",) $(if $(SUITE),--suite "$(SUITE)",)

ollama-answer-benchmark-preview: ## 离线预览 Ollama 合成问答请求与冻结条件（CASE、SUITE）
	node scripts/ollama-answer-benchmark.mjs preview "$(CASE)" $(if $(MODEL),--model "$(MODEL)",) $(if $(SUITE),--suite "$(SUITE)",)

ollama-answer-diagnose: ## 显式诊断一题合成请求，只记录固定故障阶段；不能计入质量门槛（CASE、SUITE）
	node scripts/ollama-answer-benchmark.mjs diagnose "$(CASE)" $(if $(MODEL),--model "$(MODEL)",) $(if $(SUITE),--suite "$(SUITE)",)

ollama-answer-quality-gate: ## 离线核对同一候选的基准/挑战/覆盖各两轮，REPORTS 为六份私有报告路径
	node scripts/ollama-answer-benchmark.mjs gate $(REPORTS)
