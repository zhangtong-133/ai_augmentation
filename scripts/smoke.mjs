// 验收流程仅使用 Node 内置模块；不操作用户的 .env 或常规 Compose 项目。
import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { randomBytes } from "node:crypto";
import { access, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";

const root = fileURLToPath(new URL("../", import.meta.url));
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

function command(binary, args, extra = {}, capture = false) {
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
    const timer = setTimeout(() => { timedOut = true; child.kill("SIGKILL"); }, 20 * 60 * 1000);
    child.on("error", error => { clearTimeout(timer); active = null; reject(error); });
    child.on("close", code => {
      clearTimeout(timer); active = null;
      if (timedOut) reject(new Error(`${binary} timed out after 20 minutes; rerun to reuse downloaded images/build caches`));
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
async function request(base, path, expected, { method = "GET", body, cookie, admin, csrf = true } = {}) {
  if (interrupted) throw new Error("interrupted");
  const headers = { "content-type": "application/json" };
  if (cookie) headers.cookie = cookie;
  if (admin) headers.authorization = `Bearer ${env.SMOKE_TOKEN}`;
  if (method === "POST" && csrf) headers["x-requested-with"] = "personal-ai";
  const response = await fetch(base + path, {
    method, headers, body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(10000), redirect: "error",
  });
  assert.equal(response.status, expected, `${method} ${path}`);
  const data = await response.json();
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
  return { email, password };
}
async function login(base, credentials) {
  const { response } = await request(base, "/api/auth/login", 200, { method: "POST", body: credentials });
  const cookie = response.headers.get("set-cookie");
  assert.ok(typeof cookie === "string", "session cookie missing");
  assert.ok(/HttpOnly/.test(cookie), "HttpOnly missing");
  assert.ok(/SameSite=Strict/.test(cookie), "SameSite missing");
  assert.ok(/Path=\/api;/.test(cookie), "cookie path incorrect");
  assert.ok(!/; Secure/.test(cookie), "HTTP smoke cookie unexpectedly Secure");
  return cookie.split(";")[0];
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

async function waitJob(base, path, cookie, predicate) {
  for (let attempt = 0; attempt < 120; attempt++) {
    const { data } = await request(base, path, 200, { cookie });
    if (predicate(data.job)) return data.job;
    assert.notEqual(data.job?.status, "failed", "background indexing failed");
    await delay(250);
  }
  throw new Error("background indexing state timeout");
}
async function verifyDurableJob(web, gateway, cookie, otherCookie) {
  const { data: doc } = await request(web, "/api/documents", 201, {
    method: "POST", cookie, body: { title: "后台索引", markdown: "# 后台任务\n\n" + "可靠索引".repeat(5000) },
  });
  assert.ok(doc.chunk_count > 16);
  const path = `/api/documents/${doc.id}/index-jobs`;
  assert.equal((await request(web, path, 200, { cookie })).data.job, null);
  await request(web, path, 401);
  await request(web, path, 401, { method: "POST" });
  await request(web, path, 403, { method: "POST", cookie, csrf: false });
  for (const method of ["GET", "POST"]) await request(gateway, path, 404, { method, cookie: otherCookie });
  await compose(["exec", "-T", "embeddings", "node", "-e",
    "fetch('http://127.0.0.1:8081/control/fail-once',{method:'POST'}).then(r=>{if(r.status!==204)process.exit(1)})"]);
  const accepted = await request(web, path, 202, { method: "POST", cookie });
  const waiting = await waitJob(gateway, path, cookie, job => job?.status === "queued" && job.error_code === "embedding_rate_limited");
  assert.equal(waiting.indexed_chunks, 0);
  assert.equal(waiting.attempts, 1);
  const duplicate = await request(gateway, path, 202, { method: "POST", cookie });
  assert.equal(accepted.data.id, duplicate.data.id);
  assert.equal(duplicate.data.attempts, 1);
  await compose(["restart", "api-server"]);
  await ready(web + "/api/readyz");
  const completed = await waitJob(web, path, cookie, job => job?.status === "succeeded");
  assert.equal(completed.id, accepted.data.id);
  assert.equal(completed.indexed_chunks, doc.chunk_count);
  assert.equal(completed.total_chunks, doc.chunk_count);
  assert.equal((await request(web, path, 200, { method: "POST", cookie })).data.id, completed.id);
  await verifyIndex(doc);
  console.log("PASS: durable whole-document job, ownership/CSRF, rate-limit backoff, API restart resume and complete vector count");
}

let started = false;
try {
  await command("docker", ["info", "--format", "{{.ServerVersion}}"], {}, true);
  try { await command("docker", ["compose", "version"], {}, true); }
  catch { binary = "docker-compose"; prefix = []; }
  console.log(`Smoke project: ${project}`);
  started = true;
  await compose(["up", "-d", "postgres"]);
  const database = (await endpoint("postgres", 5432)).replace("http://", "");
  await compose(["exec", "-T", "postgres", "sh", "-c",
    "attempt=0; until pg_isready -h 127.0.0.1 -U smoke -d smoke; do attempt=$((attempt + 1)); [ $attempt -lt 90 ] || exit 1; sleep 1; done"]);
  await command("make", ["test-postgres"], {
    TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
  });
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
  const cookie = await login(web, owner);
  const otherCookie = await login(gateway, other);
  await request(web, "/api/overview", 401);
  const empty = await request(web, "/api/overview", 200, { cookie });
  assert.equal(empty.data.knowledge.total_documents, 0);
  const body = { title: "验收笔记", markdown: "# 验收\n\n" + "知识积累。".repeat(500), tags: ["smoke"] };
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
  if (process.argv.includes("--index")) {
    await request(web, path + "/index", 403, { method: "POST", cookie, csrf: false });
    await request(gateway, path + "/index", 404, { method: "POST", cookie: otherCookie });
    for (const base of [web, gateway]) {
      const indexed = await request(base, path + "/index", 200, { method: "POST", cookie });
      assert.equal(indexed.data.indexed_chunks, document.chunk_count);
      assert.equal(indexed.data.next_offset, null);
    }
    await verifyIndex(document);
    await verifyDurableJob(web, gateway, cookie, otherCookie);
    console.log("PASS: authenticated indexing through both HTTP entry points, CSRF, isolation and idempotency");
  }
  console.log("PASS: gateway/proxy, login, CSRF, import, deduplication, isolation and overview");
  if (process.argv.includes("--objects")) await compose(["restart", "minio"]);
  if (process.argv.includes("--index")) {
    await compose(["restart", "qdrant"]);
    await ready((await endpoint("qdrant", 6333)) + "/readyz");
  }
  await compose(["restart", "postgres"]);
  await compose(["restart", "api-server"]);
  await ready(web + "/api/readyz");
  assert.equal((await request(web, path, 200, { cookie })).data.markdown, body.markdown);
  await request(web, "/api/auth/logout", 200, { method: "POST", cookie });
  await request(gateway, "/api/auth/me", 401, { cookie });
  await request(web, "/api/documents", 401, { cookie });
  const renewed = await login(web, owner);
  assert.equal((await request(web, path, 200, { cookie: renewed })).data.id, document.id);
  if (process.argv.includes("--index")) await verifyIndex(document);
  console.log("PASS: database/API restart persistence, logout revocation and re-login");
  if (process.argv.includes("--browser")) {
    // API 重启时，Docker 可能重新分配临时宿主端口。
    const browserApi = await endpoint("api-server", 8080);
    await ready(browserApi + "/api/readyz");
    await command("npm", ["--prefix", "tests/browser", "test"], {
      E2E_API_URL: browserApi, E2E_WEB_URL: web, E2E_GATEWAY_URL: gateway,
      E2E_ADMIN_TOKEN: env.SMOKE_TOKEN,
      E2E_PUBLIC_WEB: process.argv.includes("--public-web") ? "1" : "0",
    });
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
