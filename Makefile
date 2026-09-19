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
	cargo test -p personal-ai-storage-postgres --test postgres --test index_jobs -- --ignored

smoke: ## 构建隔离 Compose 环境，验证持久化与 HTTP 流程，并清理测试数据
	node scripts/smoke.mjs

browser-install: ## 安装锁定的 Playwright 依赖与当前平台的无头 Chromium
	npm --prefix tests/browser ci
	npm --prefix tests/browser run install-browser

browser-test: ## 在隔离 Compose 环境中运行 HTTP 与无头浏览器验收
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
