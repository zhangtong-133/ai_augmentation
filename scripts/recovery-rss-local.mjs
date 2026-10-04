// Explicit synthetic RSS acceptance; never collects external feeds or uses account credentials.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { randomUUID } from "node:crypto";
import { join } from "node:path";
import { pgQuery, root } from "./recovery-lib.mjs";
import { login, request } from "./recovery-http.mjs";
const execute = promisify(execFile);
const endpoint = "http://127.0.0.1:11435", model = "qwen3:4b-q4_K_M";
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
function cli(app, owner, id, target = endpoint) {
  return execute(join(root, "target/debug/local-value"), ["run", owner, id, target, model, "--use-local"],
    { cwd: root, env: { PATH: process.env.PATH, DATABASE_URL: app.database }, timeout: 10000, maxBuffer: 1048576 });
}
async function approve(app, cookie) {
  const id = randomUUID();
  const draft = (await request(app, "/api/feed-values/local", 200, { method: "POST", cookie, body: { id, endpoint, model } })).data;
  assert.equal(draft.pricing.profile, "local-rss-v4");
  assert.ok(draft.shared_content.instructions.includes("category"));
  assert.equal(draft.pricing.kind, "local"); assert.equal(draft.pricing.endpoint, endpoint); assert.equal(draft.pricing.model, model);
  assert.equal(JSON.stringify(draft.shared_content).includes("https://example.com"), false);
  await request(app, `/api/feed-values/${id}/approve-local`, 200, { method: "POST", cookie,
    body: { digest: draft.digest, acknowledge_sharing: true, acknowledge_local_compute: true } });
  return id;
}
async function sends(database, id) {
  return pgQuery(database, `SELECT count(*) FROM feed_value_audit WHERE request_id='${id}' AND event='sending';`);
}
export async function exerciseLocalRss(app, fixture, database) {
  const subscription = randomUUID();
  await request(app, "/api/feed-subscriptions", 201, { method: "POST", cookie: fixture.cookie,
    body: { id: subscription, name: "合成 RSS 评分", source_url: "https://example.com/scoring", enabled: true } });
  await request(app, "/api/feed-brief-preferences", 200, { method: "PUT", cookie: fixture.cookie, body: { revision: "0", keywords: ["rust"] } });
  const preferences = (await request(app, "/api/feed-brief-preferences", 200, { cookie: fixture.cookie })).data;
  // Only source fixtures use SQL: inference and all authorization/results use actual HTTP/CLI paths.
  for (const [i, title, summary] of [[1, "Rust 所有权", "解释 String 移动后旧绑定不可再用，给出可运行示例。"], [2, "周末天气", "天气预报，与 Rust 技能目标无关。"], [3, "Rust 新闻", "只有标题，没有可核验的技术内容。"]]) {
    const digest = String(i).repeat(64);
    await pgQuery(database, `INSERT INTO feed_entries(user_id,subscription_id,entry_key,title,summary,content_digest,first_seen_ms,updated_ms,last_seen_ms) VALUES('${fixture.owner.id}','${subscription}','guid:${digest}','${title}','${summary}','${digest}',(extract(epoch from clock_timestamp())*1000)::bigint,(extract(epoch from clock_timestamp())*1000)::bigint,(extract(epoch from clock_timestamp())*1000)::bigint);`);
  }
  const id = await approve(app, fixture.cookie);
  await assert.rejects(cli(app, fixture.owner.id, id, "http://127.0.0.1:12999"), error => error.code === 1);
  assert.equal((await request(app, `/api/feed-values/${id}`, 200, { cookie: fixture.cookie })).data.status, "authorized");
  assert.equal(await sends(database, id), "0");
  const options = { cwd: root, env: { PATH: process.env.PATH, HOME: process.env.HOME, RUSTUP_TOOLCHAIN: process.env.RUSTUP_TOOLCHAIN, DATABASE_URL: app.database }, timeout: 90000, maxBuffer: 1048576 };
  const args = ["scripts/local-model.mjs", "value", fixture.owner.id, id];
  let first;
  try { first = JSON.parse((await execute("node", args, options)).stdout); }
  catch (error) {
    let status = "unconfirmed";
    try {
      const value = JSON.parse(error.stdout);
      if (["unknown", "expired", "invalidated", "authorized", "running", "cancelled"].includes(value.status)) status = value.status;
    } catch { /* Never echo child output, model text or credentials. */ }
    console.log(JSON.stringify({ local_rss_acceptance: status, process_killed: error.killed === true,
      exit_code: Number.isInteger(error.code) ? error.code : null }));
    throw new Error("真实本地 RSS 评分未确认成功；核对原请求，验收不会自动重发");
  }
  assert.equal(first.status, "succeeded"); assert.equal(first.scores.length, 3);
  assert.ok(first.scores.every(s => [null, 0, 40, 80].includes(s.score)), "v4 fixed classification mapping");
  const replay = JSON.parse((await execute("node", args, options)).stdout);
  assert.equal(replay.status, "succeeded"); assert.ok(same(replay.scores, first.scores), "terminal score replay");
  assert.equal(await sends(database, id), "1");
  const reading = (await request(app, `/api/feed-values/${id}/reading`, 200, { cookie: fixture.cookie })).data;
  assert.equal(reading.items.length, 3); assert.ok(reading.items.every(item => first.scores.some(score => score.id === item.id && score.score === item.model_score && score.reason === item.reason)));
  await request(app, `/api/feed-values/${id}/reading`, 404, { cookie: fixture.otherCookie });
  assert.ok(same((await request(app, "/api/feed-brief-preferences", 200, { cookie: fixture.cookie })).data, preferences), "rules remain unchanged");
  const pendingId = await approve(app, fixture.cookie);
  fixture.rss = { id, pendingId, subscription, scores: first.scores, reading };
  console.log("PASS: real protected local RSS scoring, exact target refusal before claim, one dispatch, strict persisted scores, owner-only reading and unchanged rules");
}
export async function verifyLocalRss(app, fixture, database) {
  const cookie = await login(app, fixture.owner), otherCookie = await login(app, fixture.other);
  const { id, pendingId, subscription, scores, reading } = fixture.rss;
  assert.equal((await request(app, "/api/feed-values/config", 200, { cookie })).data.local_enabled, false);
  assert.ok(same((await request(app, `/api/feed-values/${id}/reading`, 200, { cookie })).data, reading), "restored reading");
  await request(app, `/api/feed-values/${id}/reading`, 404, { cookie: otherCookie });
  assert.ok(same(JSON.parse((await cli(app, fixture.owner.id, id)).stdout).scores, scores), "restored terminal replay");
  assert.equal((await request(app, `/api/feed-values/${pendingId}`, 200, { cookie })).data.status, "invalidated");
  await assert.rejects(cli(app, fixture.owner.id, pendingId), error => error.code === 1 && JSON.parse(error.stdout).status === "invalidated");
  assert.equal(await sends(database, id), "1"); assert.equal(await sends(database, pendingId), "0");
  await request(app, `/api/feed-subscriptions/${subscription}`, 200, { method: "DELETE", cookie, body: { revision: "1" } });
  const revoked = (await request(app, `/api/feed-values/${id}`, 200, { cookie })).data;
  assert.equal(revoked.status, "invalidated"); assert.equal(revoked.scores, null); assert.equal(revoked.shared_content, null);
  await request(app, `/api/feed-values/${id}/reading`, 409, { cookie });
  await assert.rejects(cli(app, fixture.owner.id, id), error => error.code === 1 && JSON.parse(error.stdout).status === "invalidated");
  assert.equal(await sends(database, id), "1");
  console.log("PASS: restored real RSS reading without inference, old pending consent quarantined, source deletion clears scores and late replay refuses resend");
}
