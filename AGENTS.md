# Repository Guidelines

## Project Structure & Module Organization

- `apps/api-server` contains the Axum API; `apps/worker` and `apps/scheduler` are background-process shells.
- `apps/web/src/app` holds Next.js routes, `src/components` holds React components, and `public/` holds static assets.
- `crates/` contains domain logic and storage, LLM, tool, and agent interfaces. Keep vendor SDKs in adapters such as `storage-postgres`; wire implementations in application entry points.
- Rust unit tests live beside code or in `src/tests.rs`; PostgreSQL integration tests and migrations live under `crates/storage-postgres/tests/` and `migrations/`.
- `infra/`, `scripts/`, and `docs/` contain deployment configuration, helpers, and designs. Update designs and the roadmap when behavior changes.

## Build, Test, and Development Commands

Use Rust 1.96+ and Node.js 20.9+ with npm.

- `make env-check`: inspect local tools and Docker availability.
- `cargo build --workspace`: compile backend applications and libraries.
- `make check`: run rustfmt checks, Clippy with warnings denied, and Rust tests.
- `cargo run -p api-server`: start the API after exporting database and authentication configuration.
- `make web-install` / `make web-dev`: install locked dependencies / start Next.js.
- `npm --prefix apps/web run lint`, `run typecheck`, and `run build`: validate frontend code and production output.
- `make compose-config` / `make infra-up`: validate Compose / start data services.
- `make smoke`: build an isolated core Compose stack, run real PostgreSQL/HTTP acceptance, then remove only its test resources. Requires local Docker access.
- `make browser-install` / `make browser-test`: install isolated Playwright/Chromium dependencies / run headless UI acceptance with the disposable Compose stack.

## Coding Style & Naming Conventions

Follow `.editorconfig`: UTF-8, LF, two-space indentation except four spaces for Rust and tabs for Make recipes. Use `cargo fmt --all`, Clippy, ESLint, and strict TypeScript. Name Rust functions/modules in snake_case, types and React components in PascalCase, and component files in kebab-case. Append numbered SQL migrations; never edit migrations already applied.

## Testing Guidelines

Use Rust's test harness, Tokio async tests, and Tower route tests. Name tests after observable behavior, covering authorization, user isolation, validation, and persistence. Run `TEST_DATABASE_URL=… make test-postgres` against a disposable database; these tests write data and are otherwise ignored. Playwright UI tests live in `tests/browser/*.spec.mjs`; run `make browser-test`. Report skipped checks explicitly. No coverage percentage is configured; also run frontend lint, typecheck, and build.

## Commit & Pull Request Guidelines

提交信息使用 `type(scope): 中文描述` 格式，scope 可省略，例如 `feat(knowledge): 添加 Markdown 导入` 或 `docs: 更新开发文档`。type（如 `feat`、`fix`、`docs`、`test`、`refactor`、`chore`、`merge`）及 scope（如 `knowledge`、`storage`）保持英文，不翻译；使用英文括号、冒号及冒号后的空格。描述以中文动词开头，正文使用中文，技术名称和代码标识符保留原文。翻译已有提交信息时，只翻译描述和正文，保留原有 type、scope 及格式；原本没有前缀的历史提交不补加前缀。历史重写和强制推送必须事先获得用户明确授权，并先备份、验证代码内容不变，再使用带明确旧 SHA 的 `--force-with-lease` 推送。

PRs should explain behavior, link issues/designs, report validation and skipped checks, describe migrations/configuration changes, and include screenshots for UI changes.

## Security & Configuration

Keep secrets out of Git and browser code. Copy `.env.example` for Compose; local Rust processes require exported variables. Use loopback database addresses locally. Disable secure cookies only for local HTTP development. Preserve owner-scoped queries and session/CSRF checks.
