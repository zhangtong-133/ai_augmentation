// 验收流程仅使用 Node 内置模块；不操作用户的 .env 或常规 Compose 项目。
import assert from "node:assert/strict";
import { learningEvents } from "./smoke-learning-events.mjs";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { randomBytes, randomUUID, createHash } from "node:crypto";
import { access, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";

const root = fileURLToPath(new URL("../", import.meta.url));
// 只接受仓库内一个明确的浏览器用例文件；不读取其他测试进程的环境。
const browserSpec = process.env.BROWSER_SPEC;
if (browserSpec !== undefined) {
  if (!process.argv.includes("--browser") || !/^[a-z][a-z0-9-]*\.spec\.mjs$/.test(browserSpec)) {
    throw new Error("BROWSER_SPEC requires --browser and a filename such as feed-values.spec.mjs");
  }
  await access(join(root, "tests/browser", browserSpec));
}
const browserSelection = browserSpec ? ["--", `(^|/)${browserSpec.replaceAll(".", "\\.")}$`] : [];

if (process.argv.includes("--public-web") && !process.argv.includes("--browser")) {
  throw new Error("--public-web requires --browser");
}
if (process.argv.includes("--browser")) {
  try {
    const require = createRequire(new URL("../tests/browser/package.json", import.meta.url));
    const { chromium } = require("@playwright/test");
    await access(chromium.executablePath());
  } catch {
    throw new Error("Missing platform-native Playwright/Chromium; run make browser-install on this machine.");
  }
}
// 隔离镜像仓库凭据前，先解析用户选定的 Docker 连接地址。
// 与 Docker CLI 一致，DOCKER_CONTEXT 优先于 DOCKER_HOST。
const execute = promisify(execFile);
const dockerHost = process.env.DOCKER_CONTEXT || !process.env.DOCKER_HOST
  ? (await execute("docker", ["context", "inspect", "--format", "{{.Endpoints.docker.Host}}"], { timeout: 10000 })).stdout.trim()
  : process.env.DOCKER_HOST;
if (!dockerHost.startsWith("unix:///")) {
  throw new Error("Smoke acceptance requires a local Unix Docker socket for loopback ports.");
}
const project = `personal-ai-smoke-${randomBytes(8).toString("hex")}`;
// 仅使用公开镜像，隔离旧 Desktop 凭据助手和登录数据。
// 保留插件发现能力（OrbStack/Desktop 会在此目录安装 buildx）。
const originalDockerConfig = process.env.DOCKER_CONFIG || join(homedir(), ".docker");
let extraPluginDirs = [];
try {
  const config = JSON.parse(await readFile(join(originalDockerConfig, "config.json"), "utf8"));
  if (Array.isArray(config.cliPluginsExtraDirs)) {
    extraPluginDirs = config.cliPluginsExtraDirs.filter(value => typeof value === "string");
  }
} catch (error) {
  if (error.code !== "ENOENT") throw new Error("Cannot read Docker plugin configuration.");
}
const dockerConfig = await mkdtemp(join(tmpdir(), "personal-ai-smoke-docker-"));
await writeFile(join(dockerConfig, "config.json"), JSON.stringify({
  auths: {}, cliPluginsExtraDirs: [...extraPluginDirs, join(originalDockerConfig, "cli-plugins")],
}), { mode: 0o600 });
const env = {
  ...process.env,
  DOCKER_CONFIG: dockerConfig,
  DOCKER_HOST: dockerHost,
  SMOKE_PASSWORD: randomBytes(24).toString("hex"),
  SMOKE_TOKEN: randomBytes(32).toString("hex"),
};
delete env.DOCKER_CONTEXT;
// 公网验收必须显式启用，不从环境中继承开关。
delete env.E2E_PUBLIC_WEB;
let active;
let interrupted = false;
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => { interrupted = true; active?.kill("SIGTERM"); });
}

function command(binary, args, extra = {}, capture = false, timeoutMinutes = 20) {
  return new Promise((resolve, reject) => {
    const child = spawn(binary, args, {
      cwd: root, env: { ...env, ...extra },
      stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
    });
    active = child;
    let output = "";
    child.stdout?.on("data", data => { output += data; });
    // 捕获的错误可能含有凭据，因此仅报告命令名和退出状态。
    child.stderr?.resume();
    let timedOut = false;
    const timer = setTimeout(() => { timedOut = true; child.kill("SIGKILL"); }, timeoutMinutes * 60 * 1000);
    child.on("error", error => { clearTimeout(timer); active = null; reject(error); });
    child.on("close", code => {
      clearTimeout(timer); active = null;
      if (timedOut) reject(new Error(`${binary} timed out after ${timeoutMinutes} minutes; inspect the command log before rerunning`));
      else if (code !== 0) reject(new Error(`${binary} exited with ${code}`));
      else resolve(output.trim());
    });
  });
}

let binary = "docker";
let prefix = ["compose"];
async function compose(args, capture = false) {
  return command(binary, [...prefix, "--project-directory", root, "--env-file", "infra/smoke.env",
    "-p", project, "-f", "compose.smoke.yaml",
    ...(process.argv.includes("--objects") ? ["-f", "compose.objects-test.yaml"] : []),
    ...(process.argv.includes("--index") ? ["-f", "compose.index-test.yaml"] : []), ...args], {}, capture);
}
async function endpoint(service, port) {
  const address = await compose(["port", service, String(port)], true);
  assert.match(address, /^127\.0\.0\.1:\d+$/);
  return `http://${address}`;
}
async function ready(url) {
  for (let attempt = 0; attempt < 90; attempt++) {
    if (interrupted) throw new Error("interrupted");
    const controller = new AbortController();
    // 保持超时计时器活跃，避免 Node 20 在首次连接服务时因无活跃句柄而退出。
    const timer = setTimeout(() => controller.abort(), 2000);
    try {
      const response = await fetch(url, { signal: controller.signal });
      await response.body?.cancel();
      if (response.ok) return;
    } catch { /* 启动期间可能暂时拒绝连接。 */ }
    finally { clearTimeout(timer); }
    await delay(1000);
  }
  throw new Error(`readiness timeout: ${url}`);
}
async function request(base, path, expected, { method = "GET", body, cookie, admin, csrf = true, requestId, token } = {}) {
  if (interrupted) throw new Error("interrupted");
  const headers = { "content-type": "application/json" };
  if (cookie) headers.cookie = cookie;
  if (token) headers.authorization = `Bearer ${token}`;
  if (admin) headers.authorization = `Bearer ${env.SMOKE_TOKEN}`;
  if (method !== "GET" && csrf) headers["x-requested-with"] = "personal-ai";
  if (requestId !== undefined) headers["idempotency-key"] = requestId;
  const response = await fetch(base + path, {
    method, headers, body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(10000), redirect: "error",
  });
  assert.equal(response.status, expected, `${method} ${path}: expected ${expected}, received ${response.status}`);
  const data = response.status === 204 ? null : await response.json();
  return { response, data };
}

async function account(api, email) {
  const password = randomBytes(20).toString("hex");
  const { data: user } = await request(api, "/api/users", 201, {
    method: "POST", admin: true, body: { email, display_name: "Smoke 用户" },
  });
  await request(api, `/api/users/${user.id}/password`, 200, {
    method: "POST", admin: true, body: { password },
  });
  return { id: user.id, email, password };
}
async function login(base, credentials) {
  const { response } = await request(base, "/api/auth/login", 200, { method: "POST", body: { email: credentials.email, password: credentials.password } });
  const cookie = response.headers.get("set-cookie");
  assert.ok(typeof cookie === "string", "session cookie missing");
  assert.ok(/HttpOnly/.test(cookie), "HttpOnly missing");
  assert.ok(/SameSite=Strict/.test(cookie), "SameSite missing");
  assert.ok(/Path=\/api;/.test(cookie), "cookie path incorrect");
  assert.ok(!/; Secure/.test(cookie), "HTTP smoke cookie unexpectedly Secure");
  return cookie.split(";")[0];
}

// Exercise the actual stdio executable against the disposable authenticated API.
async function mcpSearch(api, token, query, requestId) {
  const messages = [
    { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "smoke", version: "1" } } },
    { jsonrpc: "2.0", method: "notifications/initialized" },
    { jsonrpc: "2.0", id: 2, method: "tools/list" },
    { jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "knowledge_search", arguments: { query, request_id: requestId, acknowledge_embedding_cost: true } } },
  ];
  const child = spawn(join(root, "target/debug/personal-ai-mcp"), [], {
    env: { ...process.env, MCP_API_URL: api, MCP_ACCESS_TOKEN: token, MCP_ALLOW_EMBEDDING_COST: "1" },
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stdout = "", stderr = "";
  child.stdout.on("data", data => { stdout += data; });
  child.stderr.on("data", data => { stderr += data; });
  const completed = new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("close", code => code === 0 ? resolve() : reject(new Error("MCP bridge failed")));
  });
  const timeout = setTimeout(() => child.kill(), 60000);
  try {
    child.stdin.end(messages.map(message => JSON.stringify(message)).join("\n") + "\n");
    await completed;
  } finally { clearTimeout(timeout); }
  assert.equal(stderr, "");
  const replies = stdout.trim().split("\n").map(line => JSON.parse(line));
  assert.equal(replies.length, 3);
  assert.equal(replies[1].result.tools[0].name, "knowledge_search");
  return replies[2].result;
}

async function verifyIndex(document) {
  const base = await endpoint("qdrant", 6333);
  const response = await fetch(base + "/collections/smoke_knowledge/points/count", {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ exact: true, filter: { must: [{ key: "record.document_id", match: { value: document.id } }] } }),
    signal: AbortSignal.timeout(5000),
  });
  assert.equal(response.status, 200);
  assert.equal((await response.json()).result.count, document.chunk_count);
}

let started = false;
try {
  await command("docker", ["info", "--format", "{{.ServerVersion}}"], {}, true);
  try { await command("docker", ["compose", "version"], {}, true); }
  catch { binary = "docker-compose"; prefix = []; }
  console.log(`Smoke project: ${project}`);
  if (process.argv.includes("--index")) await command("cargo", ["build", "-p", "personal-ai-mcp", "--bin", "personal-ai-mcp"]);
  started = true;
  if (process.argv.includes("--objects")) {
    console.log("Building pinned MinIO/mc sources and checking public base image access");
    await compose(["build", "--pull", "minio", "minio-init"]);
  }
  await compose(["up", "-d", "postgres", "redis"]);
  const database = (await endpoint("postgres", 5432)).replace("http://", "");
  await compose(["exec", "-T", "postgres", "sh", "-c",
    "attempt=0; until pg_isready -h 127.0.0.1 -U smoke -d smoke; do attempt=$((attempt + 1)); [ $attempt -lt 90 ] || exit 1; sleep 1; done"]);
  await command("make", ["test-postgres"], {
    TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
  });
  await command("cargo", ["test", "-p", "scheduler", "--test", "process", "--", "--ignored"], {
    TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
  });
  await command("cargo", ["test", "-p", "api-server", "--lib", "answer_authorization_tests", "--", "--ignored"], {
    TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
  });
  await command("make", ["test-replies"], {
    TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
  });
  await command("cargo", ["test", "-p", "api-server", "--lib", "agent_plans", "--", "--ignored"], {
    TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
  });
  await command("make", ["test-feed-operations"], {
    TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
  });
  await command("make", ["test-learning-operations"], {
    TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
  });
  await command("make", ["test-feeds"], {
    TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
  });
  const redisAddress = (await endpoint("redis", 6379)).replace("http://", "");
  await command("make", ["test-learning"], {
    LEARNING_LOCAL_ENABLED: "true",
    TEST_REDIS_URL: `redis://${redisAddress}/0`,
    TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
  });
  await command("make", ["test-subscription-connections"], {
    RSS_LOCAL_ENABLED: "true",
    TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
  });
  await command("cargo", ["test", "-p", "api-server", "--bin", "chatgpt-connect", "--", "--ignored"], {
    TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
  });
  await command("make", ["test-redis"], { TEST_REDIS_URL: `redis://${redisAddress}/0` });
  if (process.argv.includes("--objects")) {
    await compose(["up", "-d", "minio"]);
    console.log("Waiting for MinIO readiness");
    await ready((await endpoint("minio", 9000)) + "/minio/health/ready");
    console.log("Initializing private MinIO bucket");
    await compose(["run", "--rm", "minio-init"]);
    await command("cargo", ["test", "-p", "personal-ai-storage-postgres", "--test", "objects", "--", "--ignored"], {
      TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
      TEST_OBJECT_ENDPOINT: await endpoint("minio", 9000),
      TEST_OBJECT_BUCKET: "originals", TEST_OBJECT_ACCESS_KEY: "smoke-user", TEST_OBJECT_SECRET_KEY: env.SMOKE_PASSWORD,
      // 测试服务始终走本机回环，避免继承宿主代理后把 S3 请求送出测试环境。
      NO_PROXY: "127.0.0.1,localhost,::1", no_proxy: "127.0.0.1,localhost,::1",
    });
  }
  if (process.argv.includes("--index")) {
    await compose(["up", "-d", "qdrant"]);
    const qdrant = await endpoint("qdrant", 6333);
    await ready(qdrant + "/readyz");
    await command("cargo", ["test", "-p", "personal-ai-storage-qdrant", "--test", "qdrant", "--", "--ignored"], {
      TEST_QDRANT_URL: qdrant, NO_PROXY: "127.0.0.1,localhost,::1", no_proxy: "127.0.0.1,localhost,::1",
    });
    await command("cargo", ["test", "-p", "api-server", "--test", "vector_maintenance", "--", "--ignored"], {
      TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
      TEST_QDRANT_URL: qdrant, NO_PROXY: "127.0.0.1,localhost,::1", no_proxy: "127.0.0.1,localhost,::1",
    });
  }
  await compose(["up", "-d", "--build"]);
  const api = await endpoint("api-server", 8080);
  const web = await endpoint("web", 3000);
  const gateway = await endpoint("nginx", 80);
  await Promise.all([api, web, gateway].map(base => ready(base + "/api/readyz")));
  for (const base of [web, gateway]) {
    const page = await fetch(base, { signal: AbortSignal.timeout(10000) });
    assert.equal(page.status, 200);
    assert.match(await page.text(), /PERSONAL AI/);
    await request(base, "/api/healthz", 200);
  }
  const owner = await account(api, "owner@smoke.example");
  const other = await account(api, "other@smoke.example");
  const auditedToolCalls = [];
  const persistedAgentPlans = [];
  const configurations = JSON.parse(await compose(["exec", "-T", "api-server", "reply-operations", "configurations"], true));
  assert.ok(Array.isArray(configurations.items));
  for (const stage of ["planning", "execution"]) {
    const modelConfigs = JSON.parse(await compose(["exec", "-T", "api-server", "model-agent-operations", "configurations", stage], true));
    assert.ok(Array.isArray(modelConfigs.items));
  }
  const learningAudit = JSON.parse(await compose(["exec", "-T", "api-server", "learning-operations", "audit", "--user", owner.id], true));
  assert.equal(learningAudit.consistent, true);
  assert.equal(learningAudit.counts.plans, 0);
  assert.equal(learningAudit.quotas.plans_today.remaining, 10);
  const learningModels = JSON.parse(await compose(["exec", "-T", "api-server", "learning-operations", "audit-models", "--user", owner.id], true));
  assert.equal(learningModels.consistent, true);
  assert.equal(learningModels.counts.authorizations, 0);
  assert.equal(learningModels.quotas.authorizations_today.remaining, 20);
  console.log("PASS: packaged learning and model authorization metadata audits");
  const feedAudit = JSON.parse(await compose(["exec", "-T", "api-server", "feed-operations", "audit", "--user", owner.id], true));
  assert.equal(feedAudit.consistent, true);
  assert.equal(feedAudit.counts.collections, 0);
  assert.equal(feedAudit.remaining_collections_today, 20);
  console.log("PASS: packaged RSS metadata audit");
  const valueAudit = JSON.parse(await compose(["exec", "-T", "api-server", "feed-operations", "audit-values", "--user", owner.id], true));
  assert.equal(valueAudit.consistent, true); assert.equal(valueAudit.counts.records, 0); assert.equal(valueAudit.remaining_previews_today, 20);
  console.log("PASS: packaged read-only RSS value metadata audit");
  assert.match(await compose(["exec", "-T", "api-server", "local-value", "--help"], true), /local-value run OWNER REQUEST ENDPOINT MODEL --use-local/);
  console.log("PASS: packaged explicit local RSS scoring command");
  const qualityManifest = JSON.parse(await compose(["exec", "-T", "api-server", "local-value-benchmark", "manifest"], true));
  assert.deepEqual(qualityManifest.map(c => c.id), ["rust_preference", "python_preference", "insufficient_content", "injected_summary"]);
  assert.ok(qualityManifest.every(c => c.execution_profile === "local-rss-v4" && c.prompt_bytes <= 5632));
  assert.match(await compose(["exec", "-T", "api-server", "local-value-benchmark", "--help"], true), /--use-local-benchmark/);
  console.log("PASS: packaged offline RSS quality corpus; no model call");
  const answerManifest = JSON.parse(await compose(["exec", "-T", "api-server", "local-answer-benchmark", "manifest", "http://127.0.0.1:11435", "qwen3:4b-q4_K_M"], true));
  assert.equal(answerManifest.length, 7);
  assert.ok(answerManifest.every(item => item.case.synthetic_only && item.request_sha256.length === 64));
  console.log("PASS: packaged offline local answer corpus and exact request preview; no model call");
  const answerChallenge = JSON.parse(await compose(["exec", "-T", "api-server", "local-answer-benchmark", "manifest", "http://127.0.0.1:11434", "qwen3.5:9b", "--suite", "challenge", "--backend", "ollama"], true));
  assert.deepEqual(answerChallenge.map(item => item.case.id), ["missing_schedule", "partial_answer", "forged_system", "exfiltration_instruction", "quoted_attack", "unicode_normalization", "source_order", "ambiguous_quote"]);
  assert.ok(answerChallenge.every(item => item.execution_profile === "ollama-knowledge-answer-v7" && item.case.suite === "knowledge-answer-challenge-v1" && item.case.synthetic_only && item.request_sha256.length === 64));
  console.log("PASS: packaged independent answer challenge and native Ollama preview; no model call");
  const answerCoverage = JSON.parse(await compose(["exec", "-T", "api-server", "local-answer-benchmark", "manifest", "http://127.0.0.1:11434", "qwen3.5:9b", "--suite", "coverage", "--backend", "ollama"], true));
  assert.deepEqual(answerCoverage.map(item => item.case.id), ["missing_budget", "complete_budget", "split_missing_owner", "split_complete_owner", "paired_missing_duration", "paired_complete_duration", "injected_missing_time", "unrelated_event_time", "complete_reordered", "coverage_security_discussion"]);
  assert.ok(answerCoverage.every(item => item.execution_profile === "ollama-knowledge-answer-v7" && item.case.suite === "knowledge-answer-coverage-v1" && item.case.synthetic_only && item.request_sha256.length === 64));
  const coveragePreview = JSON.parse(await compose(["exec", "-T", "api-server", "local-answer-benchmark", "preview", "missing_budget", "http://127.0.0.1:11434", "qwen3.5:9b", "--suite", "coverage", "--backend", "ollama"], true));
  assert.deepEqual(Object.keys(coveragePreview.preview.body.format.properties), ["excerpts"]);
  assert.deepEqual(coveragePreview.manifest, answerCoverage[0]);
  console.log("PASS: packaged coverage controls and exact extractive selection preview; no model call");
  const answerExtraction = JSON.parse(await compose(["exec", "-T", "api-server", "local-answer-benchmark", "manifest", "http://127.0.0.1:11434", "qwen3.5:9b", "--suite", "extraction", "--backend", "ollama"], true));
  assert.deepEqual(answerExtraction.map(item => item.case.id), ["same_source_injection", "same_source_missing", "missing_member", "complete_members", "absence_is_not_fact", "extract_security_example", "multiline_unicode", "other_team_time"]);
  assert.ok(answerExtraction.every(item => item.execution_profile === "ollama-knowledge-answer-v7" && item.case.suite === "knowledge-answer-extraction-v1" && item.case.synthetic_only && item.request_sha256.length === 64));
  console.log("PASS: packaged independent extractive controls; no model call");
  const challengeManifest = JSON.parse(await compose(["exec", "-T", "api-server", "local-value-benchmark", "manifest", "--suite", "challenge"], true));
  assert.deepEqual(challengeManifest.map(c => c.id), ["short_substantive", "ambiguous_word", "keyword_stuffing", "quoted_security", "forged_conversation", "mixed_abstention"]);
  assert.ok(challengeManifest.every(c => c.quality_version === "rss-challenge-v1" && c.execution_profile === "local-rss-v4" && c.prompt_bytes <= 5632));
  assert.notEqual(challengeManifest[0].corpus_sha256, qualityManifest[0].corpus_sha256);
  console.log("PASS: packaged independent RSS challenge suite; no model call");
  const regressionManifest = JSON.parse(await compose(["exec", "-T", "api-server", "local-value-benchmark", "manifest", "--suite", "regression"], true));
  assert.equal(regressionManifest.length, 1);
  assert.equal(regressionManifest[0].id, "recovery_insufficient");
  assert.equal(regressionManifest[0].execution_profile, "local-rss-v4");
  assert.equal(regressionManifest[0].quality_version, "rss-regression-v1");
  console.log("PASS: packaged nonempty insufficient-content regression; no model call");
  const orderManifest = JSON.parse(await compose(["exec", "-T", "api-server", "local-value-benchmark", "manifest", "--suite", "order"], true));
  assert.deepEqual(orderManifest.map(c => c.id), ["order_1", "order_2", "order_3", "order_4", "order_5", "order_6"]);
  assert.ok(orderManifest.every(c => c.quality_version === "rss-order-v1" && c.execution_profile === "local-rss-v4" && c.prompt_bytes <= 5632));
  assert.equal(new Set(orderManifest.map(c => JSON.stringify(c.items))).size, 6);
  console.log("PASS: packaged six actual RSS permutations; no model call");
  for (const suite of ["public_calibration", "public_holdout"]) {
    const manifest = JSON.parse(await compose(["exec", "-T", "api-server", "local-value-benchmark", "manifest", "--suite", suite], true));
    assert.deepEqual(manifest.map(c => c.id), ["rust_topic", "python_topic", "linux_topic", suite === "public_calibration" ? "memory_partial" : "borrowing_paraphrase"]);
    assert.ok(manifest.every(c => c.quality_version === `rss-${suite.replace("_", "-")}-v1` && c.material_origin === "public_document_paraphrase" && c.execution_profile === "local-rss-v4" && c.prompt_bytes <= 5632));
  }
  console.log("PASS: packaged source-derived calibration and holdout fixtures; no model call");
  const mcpAudit = JSON.parse(await compose(["exec", "-T", "api-server", "mcp-operations", "audit", "--user", owner.id], true));
  assert.equal(mcpAudit.consistent, true);
  assert.equal(mcpAudit.counts.total, 0);
  assert.equal(mcpAudit.remaining_issuance, 20);
  console.log("PASS: packaged MCP credential metadata audit");
  const scheduleAudit = JSON.parse(await compose(["exec", "-T", "api-server", "scheduler-operations", "audit", "--user", owner.id], true));
  assert.equal(scheduleAudit.consistent, true);
  assert.equal(scheduleAudit.counts.tasks, 0);
  assert.equal(scheduleAudit.worker_liveness, "unknown");
  const modelAudit = JSON.parse(await compose(["exec", "-T", "api-server", "model-agent-operations", "ledger",
    "--user", owner.id, "--day", new Date().toISOString().slice(0, 10), "--currency", "USD"], true));
  assert.equal(modelAudit.consistent, true);
  const audit = JSON.parse(await compose(["exec", "-T", "api-server", "reply-operations", "ledger",
    "--user", owner.id, "--day", new Date().toISOString().slice(0, 10), "--currency", "USD"], true));
  assert.equal(audit.consistent, true);
  assert.equal(audit.counts.requests, 0);
  assert.equal(audit.totals.expected_occupied_micro, "0");
  console.log("PASS: packaged reply operations configuration queries and owner-scoped ledger audit");
  const cookie = await login(web, owner);
  const otherCookie = await login(gateway, other);
  await request(web, "/api/overview", 401);
  const empty = await request(web, "/api/overview", 200, { cookie });
  assert.equal(empty.data.knowledge.total_documents, 0);
  for (const base of [web, gateway]) {
    const connection = randomUUID();
    // Trusted fixture metadata only; no OAuth credentials or external model calls.
    await compose(["exec", "-T", "postgres", "psql", "-U", "smoke", "-d", "smoke", "-v", "ON_ERROR_STOP=1", "-c",
      `INSERT INTO subscription_connections(user_id,id,host_id,client_id,subject_hash,label,models,revision,status,expires_ms)
       VALUES('${owner.id}','${connection}','${randomUUID()}','oaiapp_${randomUUID()}','${"a".repeat(64)}','smoke','["fixture"]',1,'active',floor(extract(epoch FROM clock_timestamp())*1000)::bigint+3000000)`]);
    await learningEvents({ base, cookie, otherCookie, connection, request, textFixture: async (id, packet, claim) => {
      if (claim) await compose(["exec", "-T", "postgres", "psql", "-U", "smoke", "-d", "smoke", "-v", "ON_ERROR_STOP=1", "-c",
        `UPDATE learning_model_authorizations SET status='running',dispatch_token='${randomUUID()}',dispatch_deadline_ms=floor(extract(epoch FROM clock_timestamp())*1000)::bigint+60000,sent_ms=floor(extract(epoch FROM clock_timestamp())*1000)::bigint WHERE user_id='${owner.id}' AND request_id='${id}' AND status='authorized'`]);
      const hash = value => createHash("sha256").update(value).digest("hex");
      await compose(["exec", "-T", "redis", "redis-cli", "PUBLISH", `learning-text:v1:${hash(owner.id)}:${hash(id)}`,
        JSON.stringify({ version: "learning-text-bridge-v1", owner: owner.id, request: id, packet })], true);
    } });
    const valueSource = randomUUID();
    await request(base, "/api/feed-subscriptions", 201, { method: "POST", cookie, body: { id: valueSource, name: "Scoring smoke", source_url: "https://example.com/scoring", enabled: true } });
    await compose(["exec", "-T", "postgres", "psql", "-U", "smoke", "-d", "smoke", "-v", "ON_ERROR_STOP=1", "-c",
      `INSERT INTO feed_brief_preferences(user_id,revision,keywords) VALUES('${owner.id}',1,'["rust"]') ON CONFLICT(user_id) DO NOTHING;
       INSERT INTO feed_entries(user_id,subscription_id,entry_key,title,summary,content_digest,first_seen_ms,updated_ms,last_seen_ms)
       SELECT '${owner.id}','${valueSource}','guid:${"b".repeat(64)}','Rust news','Learning Rust','${"b".repeat(64)}',t,t,t FROM (SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint AS t) now`]);
    const valueInput = { id: randomUUID(), connection_id: connection, connection_revision: "1", model: "fixture" };
    await request(base, "/api/feed-values", 401);
    await request(base, "/api/feed-values", 403, { method: "POST", cookie, csrf: false, body: valueInput });
    await request(base, "/api/feed-values", 404, { method: "POST", cookie: otherCookie, body: valueInput });
    await request(base, "/api/feed-values", 422, { method: "POST", cookie, body: { ...valueInput, amount: "0" } });
    const value = (await request(base, "/api/feed-values", 200, { method: "POST", cookie, body: valueInput })).data;
    assert.equal(value.status, "draft"); assert.equal(value.execution_mode, "local_only");
    assert.equal(typeof value.expires_at_unix_ms, "string"); assert(value.shared_content.input.includes("Rust"));
    const valuePath = `/api/feed-values/${value.id}`;
    await request(base, valuePath, 404, { cookie: otherCookie });
    const valueConsent = { digest: value.digest, acknowledge_sharing: true, acknowledge_subscription_usage: true };
    assert.equal((await request(base, `${valuePath}/approve`, 200, { method: "POST", cookie, body: valueConsent })).data.status, "authorized");
    await request(base, `${valuePath}/run`, 404, { method: "POST", cookie, body: {} });
    const values = (await request(base, "/api/feed-values", 200, { cookie })).data;
    assert(values.items.some(item => item.id === value.id));
    assert(values.items.every(item => !("shared_content" in item)));
    const cancelledValue = (await request(base, `${valuePath}/cancel`, 200, { method: "POST", cookie, body: {} })).data;
    assert.equal(cancelledValue.status, "cancelled"); assert.equal(cancelledValue.shared_content, null);
    assert.equal((await request(base, `${valuePath}/audit`, 200, { cookie })).data.items.at(-1).event, "cancelled");
    await request(base, `${valuePath}/approve`, 409, { method: "POST", cookie, body: valueConsent });
    const readingValue = (await request(base, "/api/feed-values", 200, { method: "POST", cookie, body: { ...valueInput, id: randomUUID() } })).data;
    const readingPath = `/api/feed-values/${readingValue.id}/reading`;
    await request(base, readingPath, 401);
    await request(base, readingPath, 404, { cookie: otherCookie });
    await request(base, readingPath, 409, { cookie });
    await request(base, `${readingPath}?order=untrusted`, 400, { cookie });
    await request(base, `/api/feed-values/${readingValue.id}/approve`, 200, { method: "POST", cookie, body: { ...valueConsent, digest: readingValue.digest } });
    // 只为读取投影设置已完成数据库夹具；真实一次性执行链路由 Rust 集成用例覆盖。
    await compose(["exec", "-T", "postgres", "psql", "-U", "smoke", "-d", "smoke", "-v", "ON_ERROR_STOP=1", "-c",
      `UPDATE feed_value_reviews SET status='succeeded', dispatch_token='${randomUUID()}', dispatch_deadline_ms=expires_ms, sent_ms=approved_ms, scores='[{"id":1,"score":80,"reason":"Rust relevance"}]' WHERE user_id='${owner.id}' AND id='${readingValue.id}'`]);
    const reading = (await request(base, readingPath, 200, { cookie })).data;
    assert.equal(reading.id, readingValue.id); assert.equal(reading.digest, readingValue.digest);
    assert.equal(reading.items[0].summary, "Learning Rust"); assert.equal(reading.items[0].model_score, 80);
    assert.equal(reading.items[0].rule_score, 52); assert.equal(typeof reading.as_of_unix_ms, "string");
    await request(base, `/api/feed-subscriptions/${valueSource}`, 200, { method: "DELETE", cookie, body: { revision: "1" } });
    const unavailableReading = (await request(base, readingPath, 409, { cookie })).data;
    assert(!JSON.stringify(unavailableReading).includes("Learning Rust"));
    const path = `/api/subscription-connections/${connection}`;
    await request(base, "/api/subscription-connections", 401);
    const page = await request(base, "/api/subscription-connections", 200, { cookie });
    assert(page.data.items.some(item => item.id === connection));
    const detail = await request(base, path, 200, { cookie });
    assert.equal(detail.data.revision, "1");
    assert.deepEqual(Object.keys(detail.data).sort(), ["id", "label", "models", "revision", "status", "valid_until_unix_ms"]);
    await request(base, path, 404, { cookie: otherCookie });
    await request(base, `${path}/revoke`, 403, { method: "POST", cookie, csrf: false, body: { revision: "1" } });
    await request(base, `${path}/revoke`, 404, { method: "POST", cookie: otherCookie, body: { revision: "1" } });
    await request(base, `${path}/revoke`, 422, { method: "POST", cookie, body: { revision: "1", access_token: "rejected" } });
    const revoked = await request(base, `${path}/revoke`, 200, { method: "POST", cookie, body: { revision: "1" } });
    assert.equal(revoked.data.status, "revoked");
    assert.equal(revoked.data.revision, "2");
    const replay = await request(base, `${path}/revoke`, 200, { method: "POST", cookie, body: { revision: "1" } });
    assert.deepEqual(replay.data, revoked.data);
  }
  console.log("PASS: owner-scoped subscription and scoring consent management through both gateways");
  for (const base of [web, gateway]) {
    await request(base, "/api/feeds/config", 401);
    const config = await request(base, "/api/feeds/config", 200, { cookie });
    assert.deepEqual(config.data, { mode: "disabled", execution_enabled: false });
    const input = { id: randomUUID(), name: "RSS smoke", source_url: "https://example.com/rss", enabled: true };
    await request(base, "/api/feed-subscriptions", 403, { method: "POST", cookie, body: input, csrf: false });
    const sub = await request(base, "/api/feed-subscriptions", 201, { method: "POST", cookie, body: input });
    assert.equal(sub.data.snapshot.revision, "1");
    const path = `/api/feed-subscriptions/${input.id}`;
    await request(base, path, 404, { cookie: otherCookie });
    const draft = await request(base, `${path}/collections`, 201, { method: "POST", cookie, body: { request_id: randomUUID() } });
    const collection = `/api/feed-collections/${draft.data.plan.request_id}`;
    const consent = { accepted_digest: draft.data.digest, acknowledge_source_request: true };
    const disabled = await request(base, `${collection}/confirm`, 503, { method: "POST", cookie, body: consent });
    assert.equal(disabled.data.error.code, "feed_execution_disabled");
    assert.equal((await request(base, collection, 200, { cookie })).data.status, "draft");
    await request(base, `${collection}/claim`, 404, { method: "POST", cookie, body: {} });
    const now = Date.now();
    const scheduleInput = { schedule_id: randomUUID(), starts_at_unix_ms: String(now + 600_000), ends_at_unix_ms: String(now + 7_800_000), interval_hours: 1 };
    await request(base, `${path}/schedules`, 403, { method: "POST", cookie, body: scheduleInput, csrf: false });
    const schedule = (await request(base, `${path}/schedules`, 201, { method: "POST", cookie, body: scheduleInput })).data;
    const schedulePath = `/api/feed-schedules/${scheduleInput.schedule_id}`;
    const scheduleConsent = { accepted_digest: schedule.digest, acknowledge_recurring_source_requests: true };
    assert.equal(schedule.plan.input.starts_at_unix_ms, scheduleInput.starts_at_unix_ms);
    await request(base, schedulePath, 401);
    await request(base, schedulePath, 404, { cookie: otherCookie });
    await request(base, `${schedulePath}/approve`, 400, { method: "POST", cookie, body: { ...scheduleConsent, acknowledge_recurring_source_requests: false } });
    const approved = await request(base, `${schedulePath}/approve`, 200, { method: "POST", cookie, body: scheduleConsent });
    assert.equal(approved.data.status, "active");
    assert.equal(approved.response.headers.get("cache-control"), "no-store");
    assert.deepEqual((await request(base, `${schedulePath}/approve`, 200, { method: "POST", cookie, body: scheduleConsent })).data, approved.data);
    assert((await request(base, "/api/feed-schedules", 200, { cookie })).data.items.some(item => item.digest === schedule.digest));
    await request(base, `${schedulePath}/audit`, 200, { cookie });
    const changed = await request(base, path, 200, { method: "PUT", cookie, body: { revision: "1", name: "Disabled", source_url: input.source_url, enabled: false } });
    assert.equal(changed.data.snapshot.revision, "2");
    assert.equal((await request(base, schedulePath, 200, { cookie })).data.status, "cancelled");
    await request(base, `${schedulePath}/cancel`, 200, { method: "POST", cookie, body: {} });
    await request(base, `${schedulePath}/approve`, 409, { method: "POST", cookie, body: scheduleConsent });
    await request(base, `${collection}/cancel`, 200, { method: "POST", cookie, body: {} });
    await request(base, `${collection}/audit`, 200, { cookie });
    await request(base, `${path}/entries`, 200, { cookie });
    await request(base, path, 200, { method: "DELETE", cookie, body: { revision: "2" } });
    await request(base, path, 404, { cookie });
  }
  console.log("PASS: RSS private management, immutable preview and disabled execution on both gateways");
  const persistedMemories = [];
  const persistedConversations = [];
  for (const base of [web, gateway]) {
    const body = { request_id: randomUUID(), title: "显式创建的对话" };
    await request(base, "/api/conversations", 401);
    await request(base, "/api/conversations", 403, { method: "POST", cookie, body, csrf: false });
    const saved = await request(base, "/api/conversations", 200, { method: "POST", cookie, body });
    assert.equal(saved.response.headers.get("cache-control"), "no-store");
    assert.deepEqual((await request(base, "/api/conversations", 200, { method: "POST", cookie, body })).data, saved.data);
    await request(base, "/api/conversations", 409, { method: "POST", cookie, body: { ...body, title: "不同内容" } });
    const path = `/api/conversations/${saved.data.id}`;
    const messagePath = `${path}/messages`;
    const messageBody = { request_id: randomUUID(), content: "持久化的用户消息" };
    await request(base, messagePath, 401);
    await request(base, messagePath, 403, { method: "POST", cookie, body: messageBody, csrf: false });
    await request(base, messagePath, 404, { method: "POST", cookie: otherCookie, body: messageBody });
    const message = (await request(base, messagePath, 200, { method: "POST", cookie, body: messageBody })).data;
    assert.equal(message.sequence, 1);
    assert.deepEqual((await request(base, messagePath, 200, { method: "POST", cookie, body: messageBody })).data, message);
    await request(base, messagePath, 409, { method: "POST", cookie, body: { ...messageBody, content: "不同内容" } });
    await request(base, messagePath, 422, { method: "POST", cookie, body: { ...messageBody, role: "assistant" } });
    await request(base, messagePath, 404, { cookie: otherCookie });
    const snapshot = (await request(base, messagePath, 200, { cookie })).data;
    assert.deepEqual(snapshot.messages, [message]);
    assert.equal(snapshot.revision, 1);
    const replyBody = { request_id: randomUUID(), expected_revision: 1 };
    const replyPath = `${path}/replies`;
    const replyDetail = `${replyPath}/${replyBody.request_id}`;
    await request(base, replyPath, 401);
    await request(base, replyPath, 404, { cookie: otherCookie });
    assert.deepEqual((await request(base, replyPath, 200, { cookie })).data, { enabled: true, mode: "fixture", quote: null, items: [] });
    await request(base, replyPath, 401, { method: "POST", body: replyBody });
    await request(base, replyPath, 403, { method: "POST", cookie, body: replyBody, csrf: false });
    await request(base, replyPath, 404, { method: "POST", cookie: otherCookie, body: replyBody });
    await request(base, replyPath, 422, { method: "POST", cookie, body: { ...replyBody, model: "external" } });
    await request(base, replyPath, 400, { method: "POST", cookie, body: { ...replyBody, expected_revision: 0 } });
    const submitted = await request(base, replyPath, 202, { method: "POST", cookie, body: replyBody });
    assert.equal(submitted.response.headers.get("cache-control"), "no-store");
    assert.equal(submitted.data.mode, "fixture");
    assert.equal(submitted.data.context, undefined);
    await request(base, replyDetail, 401);
    await request(base, replyDetail, 404, { cookie: otherCookie });
    await request(base, `${replyDetail}/cancel`, 403, { method: "POST", cookie, csrf: false });
    await request(base, `${replyDetail}/cancel`, 404, { method: "POST", cookie: otherCookie });
    let finished;
    for (let attempt = 0; attempt < 50; attempt++) {
      finished = (await request(base, replyDetail, 200, { cookie })).data;
      if (finished.status === "succeeded") break;
      await delay(200);
    }
    assert.equal(finished.status, "succeeded");
    assert.deepEqual((await request(base, replyPath, 200, { cookie })).data.items, [finished]);
    assert.equal(finished.output, `本地测试回复（非模型生成）：${messageBody.content}`);
    assert.deepEqual((await request(base, replyPath, 202, { method: "POST", cookie, body: replyBody })).data, finished);
    await request(base, replyPath, 409, { method: "POST", cookie, body: { ...replyBody, expected_revision: 2 } });
    assert.deepEqual((await request(base, `${replyDetail}/cancel`, 200, { method: "POST", cookie })).data, finished);
    assert.deepEqual((await request(base, messagePath, 200, { cookie })).data, snapshot);
    await request(base, path, 404, { cookie: otherCookie });
    await request(base, path, 404, { method: "DELETE", cookie: otherCookie });
    await request(base, path, 403, { method: "DELETE", cookie, csrf: false });
    assert.deepEqual((await request(base, path, 200, { cookie })).data, saved.data);
    assert.deepEqual((await request(base, "/api/conversations", 200, { cookie: otherCookie })).data, []);
    persistedConversations.push({ body, saved: saved.data, messageBody, message });
  }
  // 缓存停机不影响已提交消息；恢复后旧版本不能遮蔽新消息。
  await compose(["stop", "redis"]);
  const firstMessagePath = `/api/conversations/${persistedConversations[0].saved.id}/messages`;
  assert.equal((await request(web, firstMessagePath, 200, { cookie })).data.messages.length, 1);
  const secondMessageBody = { request_id: randomUUID(), content: "Redis 停机期间追加" };
  await request(web, firstMessagePath, 200, { method: "POST", cookie, body: secondMessageBody });
  await compose(["start", "redis"]);
  const refreshed = (await request(gateway, firstMessagePath, 200, { cookie })).data;
  assert.equal(refreshed.revision, 2);
  assert.equal(refreshed.messages.length, 2);
  for (const base of [web, gateway]) {
    const input = { title: "沟通偏好", content: "请使用中文" };
    await request(base, "/api/memories", 401);
    await request(base, "/api/memories", 403, { method: "POST", cookie, body: input, csrf: false });
    const saved = await request(base, "/api/memories", 201, { method: "POST", cookie, body: input });
    assert.equal(saved.response.headers.get("cache-control"), "no-store");
    const memoryPath = `/api/memories/${saved.data.id}`;
    const edit = { ...input, content: "中文且简洁", version: saved.data.version };
    await request(base, memoryPath, 403, { method: "PUT", cookie, body: edit, csrf: false });
    await request(base, memoryPath, 404, { method: "PUT", cookie: otherCookie, body: edit });
    const updated = await request(base, memoryPath, 200, { method: "PUT", cookie, body: edit });
    await request(base, memoryPath, 409, { method: "PUT", cookie, body: edit });
    await request(base, memoryPath, 403, { method: "DELETE", cookie, body: { version: 2 }, csrf: false });
    await request(base, memoryPath, 404, { method: "DELETE", cookie: otherCookie, body: { version: 2 } });
    await request(base, memoryPath, 409, { method: "DELETE", cookie, body: { version: 1 } });
    assert.deepEqual((await request(base, "/api/memories", 200, { cookie: otherCookie })).data, []);
    persistedMemories.push(updated.data);
  }
  // Each chunk needs distinguishable text so a <=400-character quote can be uniquely located.
  const body = { title: "验收笔记", markdown: "# 验收\n\n" + Array.from({ length: 500 }, (_, i) => `知识积累第${i + 1}条。`).join(""), tags: ["smoke"] };
  await request(web, "/api/documents", 403, { method: "POST", cookie, body, csrf: false });
  const { data: document } = await request(web, "/api/documents", 201, { method: "POST", cookie, body });
  assert.ok(document.chunk_count > 1);
  await request(gateway, "/api/documents", 409, { method: "POST", cookie, body });
  const path = `/api/documents/${document.id}`;
  await request(gateway, path, 404, { cookie: otherCookie });
  const otherOverview = await request(gateway, "/api/overview", 200, { cookie: otherCookie });
  assert.equal(otherOverview.data.knowledge.total_documents, 0);
  for (const base of [web, gateway]) {
    const details = await request(base, path, 200, { cookie });
    assert.equal(details.data.markdown, body.markdown);
    assert.equal(details.response.headers.get("cache-control"), "no-store");
    const list = await request(base, "/api/documents", 200, { cookie });
    assert.equal(list.data.length, 1);
    const overview = await request(base, "/api/overview", 200, { cookie });
    assert.equal(overview.data.knowledge.total_documents, 1);
    assert.equal(overview.data.knowledge.total_chunks, document.chunk_count);
    // 根据返回的当天时间区间计算预期值，避免 UTC 午夜切换导致测试偶发失败。
    assert.equal(overview.data.knowledge.imported_today,
      Number(document.created_at_unix_ms >= overview.data.day_start_unix_ms &&
        document.created_at_unix_ms < overview.data.day_end_unix_ms));
  }
  for (const base of [web, gateway]) {
    const path = "/api/tools/file_reader";
    const input = { document_id: document.id, offset: 2, limit: 7 };
    const requestId = randomUUID();
    const manifest = (await request(base, "/api/tools", 200, { cookie })).data;
    const reader = manifest.tools.find(tool => tool.name === "file_reader");
    assert.equal(reader.may_incur_cost, false);
    assert.equal(reader.read_only, true);
    await request(base, path, 401, { method: "POST", body: input, requestId });
    await request(base, path, 403, { method: "POST", cookie, body: input, requestId, csrf: false });
    await request(base, path, 400, { method: "POST", cookie, body: { path: "/etc/passwd" }, requestId });
    await request(base, path, 400, { method: "POST", cookie, body: input });
    const result = await request(base, path, 200, { method: "POST", cookie, body: input, requestId });
    assert.equal(result.data.output.text, Array.from(body.markdown).slice(2, 9).join(""));
    assert.equal(result.data.output.next_offset, 9);
    assert.equal(result.data.call.status, "succeeded");
    assert.equal(result.response.headers.get("cache-control"), "no-store");
    await request(base, path, 409, { method: "POST", cookie, body: input, requestId });
    await request(base, path, 403, { method: "POST", cookie: otherCookie, body: input, requestId: randomUUID() });
    const audit = (await request(base, "/api/tool-calls/" + requestId, 200, { cookie })).data;
    assert.equal(audit.tool, "file_reader");
    assert.equal(JSON.stringify(audit).includes("知识积累"), false);
    await request(base, "/api/tool-calls/" + requestId, 404, { cookie: otherCookie });
  }
  console.log("PASS: FileReader Unicode pages, no-model manifest, CSRF, isolation and one-time audit on both gateways");
  if (process.argv.includes("--index")) {
    await request(web, path + "/index", 403, { method: "POST", cookie, csrf: false });
    await request(gateway, path + "/index", 404, { method: "POST", cookie: otherCookie });
    for (const base of [web, gateway]) {
      const indexed = await request(base, path + "/index", 200, { method: "POST", cookie });
      assert.equal(indexed.data.indexed_chunks, document.chunk_count);
      assert.equal(indexed.data.next_offset, null);
    }
    const { data: searchable } = await request(web, path, 200, { cookie });
    const credentialPath = "/api/mcp/credentials";
    const grant = { host_name: "Smoke MCP host", expires_in_days: 1, acknowledge_embedding_cost: true };
    await request(web, credentialPath, 401, { method: "POST", body: grant });
    await request(web, credentialPath, 403, { method: "POST", cookie, csrf: false, body: grant });
    await request(web, credentialPath, 400, { method: "POST", cookie, body: { ...grant, scope: "admin" } });
    await request(web, credentialPath, 400, { method: "POST", cookie, body: { ...grant, acknowledge_embedding_cost: false } });
    const issued = await request(web, credentialPath, 201, { method: "POST", cookie, body: grant });
    const otherIssued = await request(gateway, credentialPath, 201, { method: "POST", cookie: otherCookie, body: grant });
    const token = issued.data.token, otherToken = otherIssued.data.token;
    assert.match(token, /^pai_mcp_[0-9a-f]{64}$/);
    assert.equal(issued.response.headers.get("cache-control"), "no-store");
    assert.equal(issued.data.credential.scope, "knowledge_search");
    const listed = await request(web, credentialPath, 200, { cookie });
    assert.equal(listed.data.items.length, 1);
    assert.equal(JSON.stringify(listed.data).includes(token), false);
    assert.equal(JSON.stringify(listed.data).includes("digest"), false);
    assert.equal((await request(web, credentialPath, 200, { cookie: otherCookie })).data.items[0].id, otherIssued.data.credential.id);
    for (const path of ["/api/auth/me", "/api/tools", "/api/tool-calls", credentialPath]) {
      await request(api, path, 401, { token });
    }
    await request(api, "/api/users", 401, { method: "POST", token, body: {} });
    await request(api, credentialPath, 401, { method: "POST", token, body: grant });
    await request(api, "/api/mcp/tools", 401, { cookie });
    await request(api, "/api/mcp/tools", 401, { token, cookie });
    await request(api, "/api/mcp/tools", 401, { token: "pai_mcp_" + "0".repeat(64) });
    const mcpTools = (await request(api, "/api/mcp/tools", 200, { token })).data.tools;
    assert.deepEqual(mcpTools.map(tool => tool.name), ["knowledge_search"]);
    await request(api, "/api/tools/file_reader", 401, { method: "POST", token, body: { document_id: document.id }, requestId: randomUUID() });
    await request(api, "/api/mcp/tools/knowledge_search", 403, { method: "POST", token, csrf: false, body: { query: "test" } });
    await request(api, "/api/mcp/tools/knowledge_search", 400, { method: "POST", token, body: { query: "test", user_id: owner.id }, requestId: randomUUID() });
    const revokePath = credentialPath + "/" + issued.data.credential.id + "/revoke";
    await request(web, revokePath, 404, { method: "POST", cookie: otherCookie });
    await request(web, revokePath, 403, { method: "POST", cookie, csrf: false });
    const mcpId = randomUUID();
    const mcp = await mcpSearch(api, token, searchable.chunks[0], mcpId);
    assert.equal(mcp.isError, false);
    assert.equal(mcp.structuredContent.hits[0].document_id, document.id);
    const mcpReplay = await mcpSearch(api, token, searchable.chunks[0], mcpId);
    assert.equal(mcpReplay.isError, true);
    assert.equal(mcpReplay.content[0].text, "request_already_used_or_conflicting");
    const mcpOther = await mcpSearch(api, otherToken, searchable.chunks[0], randomUUID());
    assert.deepEqual(mcpOther.structuredContent.hits, []);
    const mcpAudit = await request(web, "/api/tool-calls/" + mcpId, 200, { cookie });
    assert.equal(mcpAudit.data.status, "succeeded");
    await request(gateway, revokePath, 200, { method: "POST", cookie });
    await request(gateway, revokePath, 200, { method: "POST", cookie });
    await request(api, "/api/mcp/tools", 401, { token });
    await request(api, "/api/mcp/tools/knowledge_search", 401, { method: "POST", token, body: { query: "test" }, requestId: randomUUID() });
    await request(api, "/api/mcp/tools", 200, { token: otherToken });
    console.log("PASS: MCP scoped credentials, CSRF, account isolation, revocation, stdio search and persistent duplicate suppression");
    for (const base of [web, gateway]) {
      const searchPath = "/api/knowledge/search";
      const body = { query: searchable.chunks[0], limit: 5 };
      await request(base, searchPath, 401, { method: "POST", body });
      await request(base, searchPath, 403, { method: "POST", cookie, body, csrf: false });
      const found = await request(base, searchPath, 200, { method: "POST", cookie, body });
      assert.ok(found.data.hits.length > 0);
      assert.equal(found.data.hits[0].document_id, document.id);
      assert.equal(found.data.hits[0].text, searchable.chunks[found.data.hits[0].ordinal]);
      assert.equal(found.response.headers.get("cache-control"), "no-store");
      const isolated = await request(base, searchPath, 200, { method: "POST", cookie: otherCookie, body });
      assert.deepEqual(isolated.data.hits, []);
      await request(base, "/api/tools", 401);
      const manifest = await request(base, "/api/tools", 200, { cookie });
      assert.equal(manifest.data.tools.length, 2);
      const searchTool = manifest.data.tools.find(tool => tool.name === "knowledge_search");
      assert.equal(searchTool.name, "knowledge_search");
      assert.equal(searchTool.read_only, true);
      assert.equal(searchTool.may_incur_cost, true);
      assert.equal(searchTool.request_id_header, "Idempotency-Key");
      assert.equal(searchTool.daily_call_limit, 100);
      const toolPath = "/api/tools/knowledge_search";
      await request(base, toolPath, 401, { method: "POST", body });
      await request(base, toolPath, 403, { method: "POST", cookie, body, csrf: false });
      await request(base, toolPath, 400, { method: "POST", cookie, body: { ...body, user_id: "forged" } });
      await request(base, toolPath, 400, { method: "POST", cookie, body });
      const requestId = randomUUID();
      const toolResult = await request(base, toolPath, 200, { method: "POST", cookie, body, requestId });
      assert.equal(toolResult.data.call.request_id, requestId);
      assert.equal(toolResult.data.call.status, "succeeded");
      const replay = await request(base, toolPath, 409, { method: "POST", cookie, body, requestId });
      assert.equal(replay.data.error.code, "tool_call_already_used");
      const audit = await request(base, "/api/tool-calls", 200, { cookie });
      assert.ok(audit.data.items.some(call => call.request_id === requestId && call.status === "succeeded"));
      assert.equal(audit.data.used, audit.data.items.length);
      assert.equal(audit.response.headers.get("cache-control"), "no-store");
      assert.equal(JSON.stringify(audit.data).includes(body.query), false);
      await request(base, "/api/tool-calls", 401);
      await request(base, "/api/tool-calls?user_id=forged", 400, { cookie });
      await request(base, "/api/tool-calls?day=2026-02-29", 400, { cookie });
      await request(base, "/api/tool-calls/" + requestId, 404, { cookie: otherCookie });
      auditedToolCalls.push({ requestId, cookie, body });
      assert.equal(toolResult.data.tool, "knowledge_search");
      assert.equal(toolResult.data.output.hits.length, found.data.hits.length);
      for (let index = 0; index < found.data.hits.length; index += 1) {
        const { score: toolScore, ...toolHit } = toolResult.data.output.hits[index];
        const { score: searchScore, ...searchHit } = found.data.hits[index];
        assert.deepEqual(toolHit, searchHit);
        // 工具 JSON 的二次序列化可能改变浮点末位；身份与正文仍严格相等。
        assert.ok(Number.isFinite(toolScore) && Math.abs(toolScore - searchScore) < 1e-6);
      }
      assert.equal(toolResult.response.headers.get("cache-control"), "no-store");
      const toolIsolated = await request(base, toolPath, 200, { method: "POST", cookie: otherCookie, body, requestId: randomUUID() });
      assert.deepEqual(toolIsolated.data.output.hits, []);
      const conversationId = persistedConversations[0].saved.id;
      const plansPath = `/api/conversations/${conversationId}/agent-plans`;
      const revision = (await request(base, `/api/conversations/${conversationId}/messages`, 200, { cookie })).data.revision;
      const planInput = { request_id: randomUUID(), expected_revision: revision,
        searches: [{ query: body.query, limit: 5 }, { query: "知识库示例", limit: 3 }, { query: "测试索引", limit: 1 }] };
      await request(base, plansPath, 401);
      await request(base, plansPath, 403, { method: "POST", cookie, body: planInput, csrf: false });
      await request(base, plansPath, 404, { method: "POST", cookie: otherCookie, body: planInput });
      await request(base, plansPath, 422, { method: "POST", cookie, body: { ...planInput, user_id: "forged" } });
      const before = (await request(base, "/api/tool-calls", 200, { cookie })).data.used;
      const preview = (await request(base, plansPath, 200, { method: "POST", cookie, body: planInput })).data;
      assert.equal(preview.status, "draft");
      assert.equal(preview.attempted, 0);
      assert.equal(preview.steps.length, 3);
      assert.equal(preview.tool_call_limit, 3);
      assert.deepEqual((await request(base, plansPath, 200, { method: "POST", cookie, body: planInput })).data, preview);
      assert.equal((await request(base, "/api/tool-calls", 200, { cookie })).data.used, before);
      const planPath = `${plansPath}/${preview.request_id}`;
      const approval = { plan_digest: preview.digest, accepted_call_limit: 3, acknowledge_embedding_cost: true };
      await request(base, `${planPath}/approve`, 403, { method: "POST", cookie, body: approval, csrf: false });
      await request(base, `${planPath}/approve`, 404, { method: "POST", cookie: otherCookie, body: approval });
      await request(base, `${planPath}/approve`, 409, { method: "POST", cookie, body: { ...approval, plan_digest: "f".repeat(64) } });
      await request(base, `${planPath}/approve`, 400, { method: "POST", cookie, body: { ...approval, acknowledge_embedding_cost: false } });
      await request(base, `${planPath}/approve`, 202, { method: "POST", cookie, body: approval });
      let completed;
      for (let attempt = 0; attempt < 100; attempt++) {
        completed = (await request(base, planPath, 200, { cookie })).data;
        if (completed.status !== "running") break;
        await delay(100);
      }
      assert.equal(completed.status, "succeeded");
      assert.equal(completed.attempted, 3);
      assert.ok(completed.steps.every(step => step.status === "succeeded"));
      assert.equal(completed.steps[0].output.hits[0].document_id, document.id);
      assert.equal(completed.steps[0].output.hits[0].text, searchable.chunks[completed.steps[0].output.hits[0].ordinal]);
      await request(base, planPath, 404, { cookie: otherCookie });
      await request(base, `${planPath}/cancel`, 404, { method: "POST", cookie: otherCookie });
      assert.equal((await request(base, "/api/tool-calls", 200, { cookie })).data.used, before + 3);
      const replayPlan = await request(base, `${planPath}/approve`, 202, { method: "POST", cookie, body: approval });
      assert.deepEqual(replayPlan.data, completed);
      assert.equal(replayPlan.response.headers.get("cache-control"), "no-store");
      await request(base, toolPath, 409, { method: "POST", cookie, body: completed.steps[0].arguments, requestId: completed.steps[0].call_id });
      const draft = (await request(base, plansPath, 200, { method: "POST", cookie, body: { ...planInput, request_id: randomUUID() } })).data;
      const cancelled = await request(base, `${plansPath}/${draft.request_id}/cancel`, 200, { method: "POST", cookie });
      assert.equal(cancelled.data.status, "cancelled");
      await request(base, `${plansPath}/${draft.request_id}/approve`, 409, { method: "POST", cookie, body: { ...approval, plan_digest: draft.digest } });
      assert.equal((await request(base, "/api/tool-calls", 200, { cookie })).data.used, before + 3);
      const list = await request(base, plansPath, 200, { cookie });
      assert.equal(list.data.enabled, true);
      assert.equal(list.data.max_tool_calls, 3);
      assert.ok(list.data.plans.some(plan => plan.request_id === completed.request_id));
      persistedAgentPlans.push({ path: planPath, approval, completed });
      const answerPath = "/api/knowledge/answer";
      const question = { query: searchable.chunks[0] };
      await request(base, answerPath, 401, { method: "POST", body: question });
      await request(base, answerPath, 403, { method: "POST", cookie, body: question, csrf: false });
      const answered = await request(base, answerPath, 200, { method: "POST", cookie, body: question });
      assert.equal(answered.data.status, "answered");
      assert.ok(answered.data.answer.length > 0);
      assert.equal(answered.data.citations.length, 1);
      const citation = answered.data.citations[0];
      assert.equal(citation.id, 1);
      assert.equal(citation.document_id, document.id);
      assert.equal(citation.text, searchable.chunks[citation.ordinal]);
      assert.equal(citation.quote, Array.from(citation.text).slice(0, 80).join(""));
      assert.equal(citation.quote_start, 0);
      assert.equal(citation.quote_end, Array.from(citation.quote).length);
      assert.equal(answered.response.headers.get("cache-control"), "no-store");
      const unanswered = await request(base, answerPath, 200, { method: "POST", cookie: otherCookie, body: question });
      assert.deepEqual(unanswered.data, { status: "insufficient_evidence", answer: null, citations: [] });
    }
    console.log("PASS: readonly Agent plan preview, exact consent, bounded execution, cancellation, audit and owner isolation through both entry points");
    await verifyIndex(document);
    console.log("PASS: authenticated indexing, verified semantic search and cited answers through both HTTP entry points, CSRF, isolation and idempotency");
    const jobPath = path + "/index-job";
    await request(web, jobPath, 401);
    await request(web, jobPath, 403, { method: "POST", cookie, csrf: false });
    await request(gateway, jobPath, 404, { method: "POST", cookie: otherCookie });
    await request(web, jobPath, 404, { cookie });
    // 模型停机时持久化任务仍可接收；恢复依赖和 API 后自动完成多批次索引。
    await compose(["stop", "embeddings"]);
    await request(web, jobPath, 202, { method: "POST", cookie });
    const waitJob = async (base, route, statuses) => {
      for (let attempt = 0; attempt < 90; attempt++) {
        const { data } = await request(base, route, 200, { cookie });
        assert.equal(data.owner, undefined);
        assert.equal(data.lease, undefined);
        if (statuses.includes(data.status)) return data;
        assert.notEqual(data.status, "failed");
        await new Promise(resolve => setTimeout(resolve, 500));
      }
      throw new Error("index job state timeout");
    };
    const retry = await waitJob(web, jobPath, ["retrying"]);
    assert.equal(retry.indexed_chunks, 0);
    assert.equal(retry.error_code, "embedding_unavailable");
    const duplicate = await request(gateway, jobPath, 202, { method: "POST", cookie });
    assert.equal(duplicate.data.attempts, retry.attempts);
    await compose(["restart", "api-server"]);
    await compose(["start", "embeddings"]);
    await Promise.all([web, gateway].map(base => ready(base + "/api/readyz")));
    const done = await waitJob(gateway, jobPath, ["completed"]);
    assert.equal(done.indexed_chunks, document.chunk_count);
    await request(gateway, jobPath, 404, { cookie: otherCookie });
    const large = await request(web, "/api/documents", 201, { method: "POST", cookie,
      body: { title: "多批次索引", markdown: "多批次知识。".repeat(3500), tags: [] } });
    assert.ok(large.data.chunk_count > 16);
    const largePath = `/api/documents/${large.data.id}/index-job`;
    await request(gateway, largePath, 202, { method: "POST", cookie });
    assert.equal((await waitJob(web, largePath, ["completed"])).indexed_chunks, large.data.chunk_count);
    await verifyIndex(large.data);
    await verifyIndex(document);
    console.log("PASS: durable jobs, bounded retry, API restart recovery, multi-batch completion and owner isolation");
  }
  console.log("PASS: gateway/proxy, login, CSRF, import, deduplication, isolation and overview");
  if (process.argv.includes("--objects")) await compose(["restart", "minio"]);
  if (process.argv.includes("--index")) {
    await compose(["restart", "qdrant"]);
    await ready((await endpoint("qdrant", 6333)) + "/readyz");
  }
  await compose(["restart", "postgres"]);
  await compose(["restart", "api-server"]);
  await Promise.all([web, gateway].map(base => ready(base + "/api/readyz")));
  for (const call of auditedToolCalls) {
    const detail = await request(gateway, "/api/tool-calls/" + call.requestId, 200, { cookie: call.cookie });
    assert.equal(detail.data.status, "succeeded");
    await request(web, "/api/tools/knowledge_search", 409, { method: "POST", ...call });
  }
  if (auditedToolCalls.length) console.log("PASS: tool invocation audit and duplicate suppression persist across API/database restarts");
  for (const plan of persistedAgentPlans) {
    assert.deepEqual((await request(gateway, plan.path, 200, { cookie })).data, plan.completed);
    assert.deepEqual((await request(web, `${plan.path}/approve`, 202, { method: "POST", cookie, body: plan.approval })).data, plan.completed);
    for (const step of plan.completed.steps) {
      assert.equal((await request(gateway, "/api/tool-calls/" + step.call_id, 200, { cookie })).data.status, "succeeded");
      await request(web, "/api/tools/knowledge_search", 409, { method: "POST", cookie, body: step.arguments, requestId: step.call_id });
    }
  }
  if (persistedAgentPlans.length) console.log("PASS: Agent plan consent, results and one-time step IDs persist across API/database restarts");
  assert.equal((await request(web, path, 200, { cookie })).data.markdown, body.markdown);
  await request(web, "/api/auth/logout", 200, { method: "POST", cookie });
  await request(gateway, "/api/auth/me", 401, { cookie });
  await request(web, "/api/documents", 401, { cookie });
  await request(web, "/api/conversations", 401, { cookie });
  await request(web, firstMessagePath, 401, { cookie });
  await request(gateway, `/api/conversations/${persistedConversations[0].saved.id}`, 401, { method: "DELETE", cookie });
  const renewed = await login(web, owner);
  for (const base of [web, gateway]) {
    assert.equal((await request(base, "/api/conversations", 200, { cookie: renewed })).data.length, 2);
    for (const { body, saved, messageBody, message } of persistedConversations) {
      assert.deepEqual((await request(base, "/api/conversations", 200, { method: "POST", cookie: renewed, body })).data, saved);
      assert.deepEqual((await request(base, `/api/conversations/${saved.id}/messages`, 200, { method: "POST", cookie: renewed, body: messageBody })).data, message);
    }
  }
  await compose(["stop", "redis"]);
  for (const { body, saved } of persistedConversations) {
    const path = `/api/conversations/${saved.id}`;
    await request(web, path, 204, { method: "DELETE", cookie: renewed });
    await request(gateway, path, 204, { method: "DELETE", cookie: renewed });
    await request(gateway, path, 404, { cookie: renewed });
    await request(web, "/api/conversations", 409, { method: "POST", cookie: renewed, body });
    await request(gateway, `${path}/messages`, 404, { cookie: renewed });
    await request(web, `${path}/messages`, 404, { method: "POST", cookie: renewed, body: secondMessageBody });
  }
  await compose(["start", "redis"]);
  // 等待持久化删除任务自动重试；只检查当前验收用户的精确缓存键。
  const digest = value => createHash("sha256").update(value).digest("hex");
  for (const { saved } of persistedConversations) {
    const cacheKey = `messages:v1:{${digest(owner.id)}}:${digest(saved.id)}`;
    let cleared = false;
    for (let attempt = 0; attempt < 30; attempt += 1) {
      const raw = await compose(["exec", "-T", "redis", "redis-cli", "--raw", "GET", cacheKey], true);
      const snapshot = raw ? JSON.parse(raw) : null;
      if (snapshot?.deleted && snapshot.messages.length === 0) { cleared = true; break; }
      await delay(1000);
    }
    assert.ok(cleared, "durable message cache deletion must recover after Redis restart");
  }
  console.log("PASS: message idempotency, Redis outage fallback, version validation and durable deletion recovery");
  assert.deepEqual((await request(gateway, "/api/conversations", 200, { cookie: renewed })).data, []);
  for (const base of [web, gateway]) {
    const memories = (await request(base, "/api/memories", 200, { cookie: renewed })).data;
    for (const fact of persistedMemories) assert.deepEqual(memories.find(item => item.id === fact.id), fact);
  }
  for (const fact of persistedMemories) await request(web, `/api/memories/${fact.id}`, 204, { method: "DELETE", cookie: renewed, body: { version: fact.version } });
  assert.deepEqual((await request(gateway, "/api/memories", 200, { cookie: renewed })).data, []);
  assert.equal((await request(web, path, 200, { cookie: renewed })).data.id, document.id);
  if (process.argv.includes("--index")) {
    await verifyIndex(document);
    assert.equal((await request(web, path + "/index-job", 200, { cookie: renewed })).data.status, "completed");
  }
  console.log("PASS: database/API restart persistence, logout revocation and re-login");
  if (process.argv.includes("--browser")) {
    // API 重启时，Docker 可能重新分配临时宿主端口。
    const browserApi = await endpoint("api-server", 8080);
    await ready(browserApi + "/api/readyz");
    await command("npm", ["--prefix", "tests/browser", "test", ...browserSelection], {
      E2E_API_URL: browserApi, E2E_WEB_URL: web, E2E_GATEWAY_URL: gateway,
      E2E_ADMIN_TOKEN: env.SMOKE_TOKEN,
      E2E_PUBLIC_WEB: process.argv.includes("--public-web") ? "1" : "0",
      E2E_INDEX: process.argv.includes("--index") ? "1" : "0",
    }, false, 40);
  }
} catch (error) {
  // 不输出请求体、环境变量、Cookie 或 Docker inspect 数据。
  console.error(`FAIL: ${error.message}`);
  process.exitCode = 1;
} finally {
  if (started) {
    try {
      // 仅清理本次随机命名的项目及测试卷，不执行全局清理。
      await compose(["down", "--volumes", "--remove-orphans"]);
      console.log(`Cleaned test containers, network and data for ${project}; build images retained.`);
    } catch { console.error(`Cleanup failed; inspect Compose project ${project}.`); process.exitCode = 1; }
  }
  await rm(dockerConfig, { recursive: true, force: true });
}
