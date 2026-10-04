import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { randomBytes, randomUUID } from "node:crypto";
import { createServer } from "node:net";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { root } from "./recovery-lib.mjs";
const execute = promisify(execFile);

export async function startApplication(database, token) {
  const portServer = createServer();
  await new Promise(resolve => portServer.listen(0, "127.0.0.1", resolve));
  const port = portServer.address().port;
  await new Promise(resolve => portServer.close(resolve));
  let exited = false;
  const child = spawn(join(root, "target/debug/api-server"), [], { cwd: root, stdio: ["ignore", "pipe", "pipe"], env: {
    PATH: process.env.PATH, DATABASE_URL: database, API_AUTH_TOKEN: token, API_HOST: "127.0.0.1", API_PORT: String(port),
    SESSION_COOKIE_SECURE: "false", LEARNING_LOCAL_ENABLED: "true", CONVERSATION_REPLY_MODE: "disabled", RSS_COLLECTION_MODE: "disabled",
  } });
  child.stdout.resume(); child.stderr.resume();
  const done = new Promise(resolve => { child.on("close", () => { exited = true; resolve(); }); child.on("error", () => { exited = true; resolve(); }); });
  const close = async () => {
    if (!exited) child.kill("SIGTERM");
    const timer = setTimeout(() => { if (!exited) child.kill("SIGKILL"); }, 2000);
    try { await done; } finally { clearTimeout(timer); }
  };
  const base = `http://127.0.0.1:${port}`;
  try {
    let ready = false;
    for (let i = 0; i < 100; i++) {
      if (exited) break;
      try { const response = await fetch(base + "/api/readyz", { signal: AbortSignal.timeout(1000) }); await response.body?.cancel(); if (response.ok) { ready = true; break; } } catch { /* startup */ }
      await delay(100);
    }
    assert.equal(ready, true, "isolated API readiness");
    return { base, close, database, token };
  } catch (error) { await close(); throw error; }
}
export async function request(app, path, expected, { method = "GET", cookie, admin = false, body } = {}) {
  const headers = { "content-type": "application/json", "x-requested-with": "personal-ai" };
  if (cookie) headers.cookie = cookie;
  if (admin) headers.authorization = `Bearer ${app.token}`;
  const response = await fetch(app.base + path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body), redirect: "error", signal: AbortSignal.timeout(10000) });
  assert.equal(response.status, expected, `${method} ${path}: status ${response.status}`);
  const data = response.status === 204 ? null : await response.json();
  return { data, response };
}
async function account(app, email) {
  const password = randomBytes(20).toString("hex");
  const { data } = await request(app, "/api/users", 201, { method: "POST", admin: true, body: { email, display_name: "合成恢复用户" } });
  await request(app, `/api/users/${data.id}/password`, 200, { method: "POST", admin: true, body: { password } });
  return { id: data.id, email, password };
}
export async function login(app, user) {
  const { response } = await request(app, "/api/auth/login", 200, { method: "POST", body: { email: user.email, password: user.password } });
  const cookie = response.headers.get("set-cookie"); assert.ok(cookie?.includes("HttpOnly")); return cookie.split(";")[0];
}
const evidenceBody = { explanation: "每个值有一个所有者，移动后旧绑定不可再使用。", work: 'let a = String::from("hello"); let b = a; println!("{}", b);',
  verification: "运行输出 hello；再次使用 a 时编译器报告使用已移动的值。", limitations: "只验证 String 移动，未覆盖借用、生命周期或并发。" };
async function task(app, cookie, skill, revision) {
  const id = randomUUID();
  const { data } = await request(app, "/api/learning/plans", 201, { method: "POST", cookie, body: { request_id: id, expected_revision: revision, budget_minutes: 30, goal_skill_ids: [skill] } });
  const path = `/api/learning/plans/${id}/tasks/${data.plan.tasks[0].task_id}`;
  await request(app, `${path}/result`, 200, { method: "POST", cookie, body: { request_id: randomUUID(), outcome: "completed", note: "合成训练记录", actual_minutes: 5 } });
  const evidenceId = randomUUID();
  const evidenceInput = { request_id: evidenceId, body: evidenceBody };
  await request(app, `${path}/evidence`, 200, { method: "POST", cookie, body: evidenceInput });
  return { path: `${path}/evidence`, plan: `/api/learning/plans/${id}`, evidenceId, evidenceInput };
}
export async function seedApplication(app) {
  const owner = await account(app, "owner@recovery.example"), other = await account(app, "other@recovery.example");
  const cookie = await login(app, owner), otherCookie = await login(app, other);
  const document = (await request(app, "/api/documents", 201, { method: "POST", cookie, body: { title: "恢复私有文档", markdown: "# 合成材料\n\nUnicode 原文验证。", tags: ["recovery"] } })).data;
  const deletedInput = { request_id: randomUUID(), title: "已删除合成会话" };
  const deleted = (await request(app, "/api/conversations", 200, { method: "POST", cookie, body: deletedInput })).data;
  await request(app, `/api/conversations/${deleted.id}`, 204, { method: "DELETE", cookie });
  const skill = randomUUID();
  await request(app, `/api/learning/skills/${skill}`, 200, { method: "PUT", cookie, body: { revision: "0", name: "Rust 所有权", enabled: true, prerequisite_ids: [] } });
  const erased = await task(app, cookie, skill, "1");
  const reviewId = randomUUID();
  const verdict = { verdict: "supported", reason: "根据合成材料完成人工核验" };
  await request(app, `${erased.path}/review`, 200, { method: "POST", cookie,
    body: { request_id: reviewId, evidence_request_id: erased.evidenceId, body: Object.fromEntries(Object.keys(evidenceBody).map(key => [key, verdict])) } });
  await request(app, `${erased.path}/review/confirm`, 200, { method: "POST", cookie,
    body: { request_id: randomUUID(), review_request_id: reviewId, expected_revision: "1", score: 68 } });
  await request(app, erased.path, 200, { method: "DELETE", cookie, body: { request_id: erased.evidenceId } });
  const revision = (await request(app, "/api/learning/snapshot", 200, { cookie })).data.revision;
  const source = await task(app, cookie, skill, revision);
  const id = randomUUID();
  const draft = (await request(app, `${source.path}/local-model-authorizations`, 200, { method: "POST", cookie,
    body: { request_id: id, endpoint: "http://127.0.0.1:11435", model: "qwen3:4b-q4_K_M" } })).data;
  const approval = { digest: draft.digest, acknowledge_sharing: true, acknowledge_local_compute: true };
  await request(app, `/api/learning/model-authorizations/${id}/approve-local`, 200, { method: "POST", cookie, body: approval });
  await request(app, `/api/documents/${document.id}`, 404, { cookie: otherCookie });
  const snapshot = (await request(app, "/api/learning/snapshot", 200, { cookie })).data;
  return { owner, other, cookie, otherCookie, document, deleted, deletedInput, source, erased, id, approval, snapshot };
}
export async function exerciseLocalModel(app, fixture) {
  fixture.nativeId = randomUUID();
  const draft = (await request(app, `${fixture.source.path}/local-model-authorizations`, 200, { method: "POST", cookie: fixture.cookie,
    body: { request_id: fixture.nativeId, endpoint: "http://127.0.0.1:11435", model: "qwen3:4b-q4_K_M" } })).data;
  await request(app, `/api/learning/model-authorizations/${fixture.nativeId}/approve-local`, 200, { method: "POST", cookie: fixture.cookie,
    body: { digest: draft.digest, acknowledge_sharing: true, acknowledge_local_compute: true } });
  const options = { cwd: root, env: { PATH: process.env.PATH, HOME: process.env.HOME, RUSTUP_TOOLCHAIN: process.env.RUSTUP_TOOLCHAIN, DATABASE_URL: app.database }, timeout: 90000, maxBuffer: 1048576 };
  const args = ["scripts/local-model.mjs", "review", fixture.owner.id, fixture.nativeId];
  let first;
  try { first = JSON.parse((await execute("node", args, options)).stdout); }
  catch { throw new Error("真实本地核验未确认成功，请核对本次原请求；验收不会自动重发"); }
  assert.equal(first.status, "succeeded"); assert.ok(first.advice);
  const replay = JSON.parse((await execute("node", args, options)).stdout);
  assert.equal(replay.status, "succeeded"); assert.deepEqual(replay.advice, first.advice);
  fixture.nativeAdvice = first.advice;
  const saved = (await request(app, `/api/learning/model-authorizations/${fixture.nativeId}`, 200, { cookie: fixture.cookie })).data;
  assert.deepEqual(saved.advice, first.advice);
  assert.deepEqual((await request(app, "/api/learning/snapshot", 200, { cookie: fixture.cookie })).data, fixture.snapshot);
  console.log("PASS: real local model from exact HTTP consent to strict persisted advice, terminal replay and unchanged assessments");
}
export async function verifyApplication(app, fixture) {
  await request(app, "/api/learning/snapshot", 401, { cookie: fixture.cookie });
  const cookie = await login(app, fixture.owner), otherCookie = await login(app, fixture.other);
  const { data } = await request(app, `/api/documents/${fixture.document.id}`, 200, { cookie });
  assert.equal(data.markdown, "# 合成材料\n\nUnicode 原文验证。");
  await request(app, `/api/documents/${fixture.document.id}`, 404, { cookie: otherCookie });
  await request(app, `/api/conversations/${fixture.deleted.id}`, 404, { cookie });
  await request(app, "/api/conversations", 409, { method: "POST", cookie, body: fixture.deletedInput });
  assert.deepEqual((await request(app, "/api/learning/snapshot", 200, { cookie })).data, fixture.snapshot);
  const plan = (await request(app, fixture.erased.plan, 200, { cookie })).data;
  assert.equal(plan.results[0].evidence.deleted, true); assert.equal(plan.results[0].evidence.body, null);
  await request(app, fixture.erased.path, 409, { method: "POST", cookie, body: fixture.erased.evidenceInput });
  const authorization = (await request(app, `/api/learning/model-authorizations/${fixture.id}`, 200, { cookie })).data;
  assert.equal(authorization.status, "invalidated");
  await request(app, `/api/learning/model-authorizations/${fixture.id}`, 404, { cookie: otherCookie });
  await request(app, `/api/learning/model-authorizations/${fixture.id}/approve-local`, 409, { method: "POST", cookie, body: fixture.approval });
  try {
    await execute(join(root, "target/debug/local-review"), ["run", fixture.owner.id, fixture.id, "http://127.0.0.1:11435", "qwen3:4b-q4_K_M", "--use-local"],
      { env: { PATH: process.env.PATH, DATABASE_URL: app.database }, timeout: 10000 });
    assert.fail("restored old authorization cannot run");
  } catch (error) { assert.equal(error.code, 1); assert.equal(JSON.parse(error.stdout).status, "invalidated"); }
  if (fixture.nativeId) {
    const args = ["run", fixture.owner.id, fixture.nativeId, "http://127.0.0.1:11435", "qwen3:4b-q4_K_M", "--use-local"];
    const options = { env: { PATH: process.env.PATH, DATABASE_URL: app.database }, timeout: 10000 };
    const saved = (await request(app, `/api/learning/model-authorizations/${fixture.nativeId}`, 200, { cookie })).data;
    assert.equal(saved.status, "succeeded"); assert.deepEqual(saved.advice, fixture.nativeAdvice);
    const replay = JSON.parse((await execute(join(root, "target/debug/local-review"), args, options)).stdout);
    assert.deepEqual(replay.advice, fixture.nativeAdvice);
    await request(app, fixture.source.path, 200, { method: "DELETE", cookie, body: { request_id: fixture.source.evidenceId } });
    const revoked = (await request(app, `/api/learning/model-authorizations/${fixture.nativeId}`, 200, { cookie })).data;
    assert.equal(revoked.status, "invalidated"); assert.equal(revoked.advice, null);
    try { await execute(join(root, "target/debug/local-review"), args, options); assert.fail("erased native advice cannot run again"); }
    catch (error) { assert.equal(error.code, 1); assert.equal(JSON.parse(error.stdout).status, "invalidated"); }
    console.log("PASS: restored real-model advice reads without dispatch and evidence deletion clears advice without resend");
  }
  console.log("PASS: restored HTTP login, owner isolation, private document, conversation/evidence erasure, assessment provenance and original authorization refusing execution");
}
