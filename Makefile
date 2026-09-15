.DEFAULT_GOAL := help

.PHONY: help env-check check fmt test test-postgres smoke browser-install browser-test web-install web-dev compose-config infra-up infra-down stack-up stack-down

help:
	@awk 'BEGIN {FS = ":.*## "} /^[a-zA-Z_-]+:.*## / {printf "%-18s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

env-check: ## Check required and optional local tools
	@./scripts/check-env.sh

check: ## Run formatting, linting, and tests for the Rust workspace
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace

fmt: ## Format Rust and web source files
	cargo fmt --all

test: ## Run Rust workspace tests
	cargo test --workspace

test-postgres: ## Run PostgreSQL integration tests using TEST_DATABASE_URL
	cargo test -p personal-ai-storage-postgres --test postgres -- --ignored

smoke: ## Build isolated Compose stack, test real persistence/HTTP flows, and clean test data
	node scripts/smoke.mjs

browser-install: ## Install locked Playwright dependencies and WSL headless Chromium
	npm --prefix tests/browser ci
	npm --prefix tests/browser run install-browser

browser-test: ## Run HTTP acceptance plus headless UI tests in an isolated Compose stack
	node scripts/smoke.mjs --browser

web-install: ## Install pinned web dependencies
	npm --prefix apps/web ci

web-dev: ## Start the Next.js development server
	npm --prefix apps/web run dev

compose-config: ## Validate the Compose model without starting services
	@./scripts/compose.sh --env-file .env.example config --quiet

infra-up: ## Start PostgreSQL, Redis, Qdrant, and MinIO
	@./scripts/compose.sh up -d postgres redis qdrant minio

infra-down: ## Stop local services
	@./scripts/compose.sh down

stack-up: ## Build and start the complete local stack
	@./scripts/compose.sh up -d --build

stack-down: ## Stop the complete local stack
	@./scripts/compose.sh down
