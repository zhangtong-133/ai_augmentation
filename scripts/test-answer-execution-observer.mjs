import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";

const require = createRequire(new URL("../apps/web/package.json", import.meta.url));
const ts = require("typescript");
const compile = async path => ts.transpileModule(await readFile(new URL(path, import.meta.url), "utf8"), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 },
}).outputText;
const moduleUrl = source => `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`;
const types = moduleUrl(await compile("../apps/web/src/components/answer-authorization-types.ts"));
const source = (await compile("../apps/web/src/lib/answer-execution-observer.ts"))
  .replace('"../components/answer-authorization-types"', JSON.stringify(types));
const { observeAnswerExecution, AnswerExecutionHttpError } = await import(moduleUrl(source));

function fixture(status = "running") {
  return {
    request_id: "00000000-0000-4000-8000-000000000123", status: "consumed", digest: "a".repeat(64),
    created_at_unix_ms: 1, expires_at_unix_ms: 600001, execution_available: true, preview: null,
    execution: { status, request_sha256: "b".repeat(64), started_at_unix_ms: 2, deadline_at_unix_ms: 90002,
      sent_at_unix_ms: status === "running" ? null : 3, finished_at_unix_ms: status === "running" ? null : 4,
      question: ["running", "succeeded"].includes(status) ? "何时开会？" : null,
      result: status === "succeeded" ? { status: "insufficient_evidence", answer: null, citations: [] } : null },
  };
}
const flush = () => new Promise(resolve => setImmediate(resolve));
function setup(t, response) {
  // Older Node 20 releases accept the timer names directly.
  try { t.mock.timers.enable({ apis: ["setTimeout"] }); }
  catch { t.mock.timers.enable(["setTimeout"]); }
  const calls = [];
  t.mock.method(globalThis, "fetch", async (url, options) => {
    options.signal.throwIfAborted(); calls.push({ url, ...options });
    return response(options);
  });
  return { calls, tick: async ms => { t.mock.timers.tick(ms); await flush(); } };
}

test("observation only reads durable status and stops after the final result", async t => {
  const current = fixture(); const received = [];
  const { calls, tick } = setup(t, () => Response.json(current));
  const observed = observeAnswerExecution(fixture(), new AbortController().signal, data => received.push(data));
  assert.equal(calls.length, 0);
  await tick(2000);
  assert.equal(received.length, 1);
  current.execution = fixture("succeeded").execution;
  await tick(2000);
  assert.equal(await observed, "terminal");
  await tick(10000);
  assert.deepEqual(received.map(data => data.execution.status), ["running", "succeeded"]);
  assert.equal(calls.length, 2);
  for (const call of calls) {
    assert.equal(call.url, `/api/knowledge/answer-authorizations/${current.request_id}`);
    assert.equal(call.method, "GET"); assert.equal(call.cache, "no-store");
    assert.equal(call.redirect, "error"); assert.equal(call.body, undefined);
  }
});

for (const status of ["failed", "unknown", "cancelled", "invalidated"]) test(`${status} stops observation without a result or another send`, async t => {
  const received = []; const { calls, tick } = setup(t, () => Response.json(fixture(status)));
  const observed = observeAnswerExecution(fixture(), new AbortController().signal, data => received.push(data));
  await tick(2000);
  assert.equal(await observed, "terminal");
  await tick(10000);
  assert.equal(calls.length, 1); assert.equal(received[0].execution.result, null);
});

test("stopping before the next read cancels its timer", async t => {
  const { calls, tick } = setup(t, () => Response.json(fixture()));
  const controller = new AbortController();
  const observed = observeAnswerExecution(fixture(), controller.signal, () => assert.fail("late status"));
  controller.abort(); assert.equal(await observed, "stopped");
  await tick(120000); assert.equal(calls.length, 0);
});

test("a body arriving after cancellation is never delivered", async t => {
  let release;
  const { calls, tick } = setup(t, () => ({ ok: true, json: () => new Promise(resolve => { release = resolve; }) }));
  const controller = new AbortController();
  const observed = observeAnswerExecution(fixture(), controller.signal, () => assert.fail("late status"));
  await tick(2000); controller.abort(); release(fixture("succeeded"));
  assert.equal(await observed, "stopped");
  await tick(10000); assert.equal(calls.length, 1);
});

test("the two minute observation limit does not invent an execution outcome", async t => {
  const { calls, tick } = setup(t, () => Response.json(fixture()));
  const observed = observeAnswerExecution(fixture(), new AbortController().signal, () => assert.fail("unexpected status"));
  await tick(120000);
  assert.equal(await observed, "timeout");
  assert.equal(calls.length, 0);
});

test("the observation deadline aborts an outstanding body read without another request", async t => {
  const { calls, tick } = setup(t, options => ({ ok: true, json: () => new Promise((_resolve, reject) => {
    options.signal.addEventListener("abort", () => reject(options.signal.reason), { once: true });
  }) }));
  const observed = observeAnswerExecution(fixture(), new AbortController().signal, () => assert.fail("unexpected status"));
  await tick(2000); assert.equal(calls.length, 1);
  await tick(118000);
  assert.equal(await observed, "timeout");
  assert.equal(calls[0].signal.aborted, true);
  await tick(10000); assert.equal(calls.length, 1);
});

for (const status of [401, 404, 429, 503]) test(`HTTP ${status} stops after one read without retrying`, async t => {
  const { calls, tick } = setup(t, () => Response.json({}, { status }));
  const rejected = assert.rejects(observeAnswerExecution(fixture(), new AbortController().signal, () => assert.fail("invalid status")),
    error => error instanceof AnswerExecutionHttpError && error.status === status);
  await tick(2000); await rejected;
  await tick(10000); assert.equal(calls.length, 1);
});

test("mismatched request identities and incomplete success bodies are never delivered", async t => {
  let current;
  const { calls, tick } = setup(t, () => Response.json(current));
  for (const mutate of [
    data => { data.request_id = "00000000-0000-4000-8000-000000000456"; },
    data => { data.digest = "c".repeat(64); },
    data => { data.execution.request_sha256 = "c".repeat(64); },
    data => { data.execution.started_at_unix_ms++; },
    data => { data.execution.deadline_at_unix_ms++; },
    data => { data.execution = null; },
    data => { data.execution.result = null; },
    data => { data.execution.question = null; },
  ]) {
    current = fixture("succeeded"); mutate(current);
    const rejected = assert.rejects(observeAnswerExecution(fixture(), new AbortController().signal, () => assert.fail("invalid status")));
    await tick(2000); await rejected;
  }
  assert.equal(calls.length, 8);
  await tick(10000); assert.equal(calls.length, 8);
});

test("network failures stop and already finished executions do not start observation", async t => {
  const { calls, tick } = setup(t, () => { throw new TypeError("network failed"); });
  const rejected = assert.rejects(observeAnswerExecution(fixture(), new AbortController().signal, () => assert.fail("invalid status")));
  await tick(2000); await rejected;
  await tick(10000); assert.equal(calls.length, 1);
  await assert.rejects(observeAnswerExecution(fixture("succeeded"), new AbortController().signal, () => {}));
  assert.equal(calls.length, 1);
});
