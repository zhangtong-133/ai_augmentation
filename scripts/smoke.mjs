// 验收流程仅使用 Node 内置模块；不操作用户的 .env 或常规 Compose 项目。
import assert from "node:assert/strict";
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
  if (method !== "GET" && csrf) headers["x-requested-with"] = "personal-ai";
  const response = await fetch(base + path, {
    method, headers, body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(10000), redirect: "error",
  });
  assert.equal(response.status, expected, `${method} ${path}`);
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
  started = true;
  await compose(["up", "-d", "postgres", "redis"]);
  const database = (await endpoint("postgres", 5432)).replace("http://", "");
  await compose(["exec", "-T", "postgres", "sh", "-c",
    "attempt=0; until pg_isready -h 127.0.0.1 -U smoke -d smoke; do attempt=$((attempt + 1)); [ $attempt -lt 90 ] || exit 1; sleep 1; done"]);
  await command("make", ["test-postgres"], {
    TEST_DATABASE_URL: `postgres://smoke:${env.SMOKE_PASSWORD}@${database}/smoke`,
  });
  const redisAddress = (await endpoint("redis", 6379)).replace("http://", "");
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
    const { data: searchable } = await request(web, path, 200, { cookie });
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
      assert.equal(manifest.data.tools.length, 1);
      assert.equal(manifest.data.tools[0].name, "knowledge_search");
      assert.equal(manifest.data.tools[0].read_only, true);
      assert.equal(manifest.data.tools[0].may_incur_cost, true);
      const toolPath = "/api/tools/knowledge_search";
      await request(base, toolPath, 401, { method: "POST", body });
      await request(base, toolPath, 403, { method: "POST", cookie, body, csrf: false });
      await request(base, toolPath, 400, { method: "POST", cookie, body: { ...body, user_id: "forged" } });
      const toolResult = await request(base, toolPath, 200, { method: "POST", cookie, body });
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
      const toolIsolated = await request(base, toolPath, 200, { method: "POST", cookie: otherCookie, body });
      assert.deepEqual(toolIsolated.data.output.hits, []);
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
      assert.equal(answered.response.headers.get("cache-control"), "no-store");
      const unanswered = await request(base, answerPath, 200, { method: "POST", cookie: otherCookie, body: question });
      assert.deepEqual(unanswered.data, { status: "insufficient_evidence", answer: null, citations: [] });
    }
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
    await command("npm", ["--prefix", "tests/browser", "test"], {
      E2E_API_URL: browserApi, E2E_WEB_URL: web, E2E_GATEWAY_URL: gateway,
      E2E_ADMIN_TOKEN: env.SMOKE_TOKEN,
      E2E_PUBLIC_WEB: process.argv.includes("--public-web") ? "1" : "0",
      E2E_INDEX: process.argv.includes("--index") ? "1" : "0",
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
