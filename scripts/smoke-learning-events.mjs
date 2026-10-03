import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";

// Real streamed HTTP through both proxies, with trusted metadata fixtures only.
export async function learningEvents({ base, cookie, otherCookie, connection, request }) {
  const skill = randomUUID();
  await request(base, `/api/learning/skills/${skill}`, 200, { method: "PUT", cookie,
    body: { revision: "0", name: "Event fixture", enabled: true, prerequisite_ids: [] } });
  const snapshot = (await request(base, "/api/learning/snapshot", 200, { cookie })).data;
  const planId = randomUUID();
  const plan = (await request(base, "/api/learning/plans", 201, { method: "POST", cookie,
    body: { request_id: planId, expected_revision: snapshot.revision, budget_minutes: 30, goal_skill_ids: [skill] } })).data;
  const task = `/api/learning/plans/${planId}/tasks/${plan.plan.tasks[0].task_id}`;
  await request(base, `${task}/result`, 200, { method: "POST", cookie,
    body: { request_id: randomUUID(), outcome: "completed", note: "Private fixture", actual_minutes: 1 } });
  await request(base, `${task}/evidence`, 200, { method: "POST", cookie,
    body: { request_id: randomUUID(), body: { explanation: "概念", work: "产物", verification: "验证", limitations: "局限" } } });
  const id = randomUUID();
  const draft = (await request(base, `${task}/evidence/model-authorizations`, 200, { method: "POST", cookie,
    body: { request_id: id, connection_id: connection, connection_revision: "1", model: "fixture" } })).data;
  const path = `/api/learning/model-authorizations/${id}`;
  await request(base, `${path}/events`, 401);
  await request(base, `${path}/events`, 404, { cookie: otherCookie });
  await request(base, `${path}/events?replay=1`, 400, { cookie });
  const replay = await fetch(base + `${path}/events`, { headers: { cookie, "last-event-id": "0" }, signal: AbortSignal.timeout(5000) });
  assert.equal(replay.status, 400);
  await replay.arrayBuffer();
  const response = await fetch(base + `${path}/events`, { headers: { cookie }, signal: AbortSignal.timeout(30000) });
  assert.equal(response.status, 200);
  assert.match(response.headers.get("content-type"), /^text\/event-stream/);
  assert.equal(response.headers.get("cache-control"), "no-store");
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let pending = "";
  async function next() {
    for (;;) {
      const boundary = pending.indexOf("\n\n");
      if (boundary >= 0) {
        const frame = pending.slice(0, boundary);
        pending = pending.slice(boundary + 2);
        const data = frame.split("\n").find((line) => line.startsWith("data: "));
        if (data) return JSON.parse(data.slice(6));
      } else {
        const { value, done } = await reader.read();
        assert.equal(done, false, "stream ended before expected event");
        pending += decoder.decode(value, { stream: true });
      }
    }
  }
  function expected(status, sequence, terminal) {
    return { protocol_version: "learning-status-v1", request_id: id, sequence,
      detail: { status, terminal } };
  }
  try {
    assert.deepEqual(await next(), expected("draft", "0", false));
    await request(base, `${path}/approve`, 200, { method: "POST", cookie,
      body: { digest: draft.digest, acknowledge_sharing: true, acknowledge_subscription_usage: true } });
    assert.deepEqual(await next(), expected("authorized", "1", false));
    // The old generic Next proxy aborted requests after ten seconds.
    await delay(11000);
    await request(base, `${path}/cancel`, 200, { method: "POST", cookie, body: {} });
    assert.deepEqual(await next(), expected("cancelled", "2", true));
    assert.equal((await reader.read()).done, true);
  } finally {
    await reader.cancel();
  }
  console.log("PASS: private learning status events, proxy streaming, no replay and cancellation");
}
