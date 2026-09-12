// Uses only Node built-ins. Never targets the user's .env or ordinary Compose project.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";

const root = fileURLToPath(new URL("../", import.meta.url));
const project = `personal-ai-smoke-${randomBytes(8).toString("hex")}`;
// Only public images are used. Isolate stale Desktop credential helpers and login data.
const dockerConfig = await mkdtemp(join(tmpdir(), "personal-ai-smoke-docker-"));
await writeFile(join(dockerConfig, "config.json"), '{"auths":{}}\n', { mode: 0o600 });
const env = {
  ...process.env,
  DOCKER_CONFIG: dockerConfig,
  DOCKER_HOST: "unix:///var/run/docker.sock",
  DOCKER_CONTEXT: "default",
  SMOKE_PASSWORD: randomBytes(24).toString("hex"),
  SMOKE_TOKEN: randomBytes(32).toString("hex"),
};
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
    // Captured errors can contain credentials; report only command name and exit code.
    child.stderr?.resume();
    const timer = setTimeout(() => child.kill("SIGKILL"), 20 * 60 * 1000);
    child.on("error", error => { clearTimeout(timer); active = null; reject(error); });
    child.on("close", code => {
      clearTimeout(timer); active = null;
      if (code !== 0) reject(new Error(`${binary} exited with ${code}`));
      else resolve(output.trim());
    });
  });
}

let binary = "docker";
let prefix = ["compose"];
async function compose(args, capture = false) {
  return command(binary, [...prefix, "--project-directory", root, "--env-file", "infra/smoke.env",
    "-p", project, "-f", "compose.smoke.yaml", ...args], {}, capture);
}
async function endpoint(service, port) {
  const address = await compose(["port", service, String(port)], true);
  assert.match(address, /^127\.0\.0\.1:\d+$/);
  return `http://${address}`;
}
async function ready(url) {
  for (let attempt = 0; attempt < 90; attempt++) {
    if (interrupted) throw new Error("interrupted");
    try {
      if ((await fetch(url, { signal: AbortSignal.timeout(2000) })).ok) return;
    } catch { /* Startup may temporarily refuse connections. */ }
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
    // Derive expectation from the returned day window to avoid UTC-midnight flakes.
    assert.equal(overview.data.knowledge.imported_today,
      Number(document.created_at_unix_ms >= overview.data.day_start_unix_ms &&
        document.created_at_unix_ms < overview.data.day_end_unix_ms));
  }
  console.log("PASS: gateway/proxy, login, CSRF, import, deduplication, isolation and overview");
  await compose(["restart", "postgres"]);
  await compose(["restart", "api-server"]);
  await ready(web + "/api/readyz");
  assert.equal((await request(web, path, 200, { cookie })).data.markdown, body.markdown);
  await request(web, "/api/auth/logout", 200, { method: "POST", cookie });
  await request(gateway, "/api/auth/me", 401, { cookie });
  await request(web, "/api/documents", 401, { cookie });
  const renewed = await login(web, owner);
  assert.equal((await request(web, path, 200, { cookie: renewed })).data.id, document.id);
  console.log("PASS: database/API restart persistence, logout revocation and re-login");
} catch (error) {
  // Do not dump request bodies, environment, cookies or Docker inspect data.
  console.error(`FAIL: ${error.message}`);
  process.exitCode = 1;
} finally {
  if (started) {
    try {
      // Only this run's randomly named project and test volumes; never global prune.
      await compose(["down", "--volumes", "--remove-orphans"]);
      console.log(`Cleaned test containers, network and data for ${project}; build images retained.`);
    } catch { console.error(`Cleanup failed; inspect Compose project ${project}.`); process.exitCode = 1; }
  }
  await rm(dockerConfig, { recursive: true, force: true });
}
