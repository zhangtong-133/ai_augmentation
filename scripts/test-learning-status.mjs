import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
const require = createRequire(new URL("../apps/web/package.json", import.meta.url));
const ts = require("typescript");
const source = await readFile(new URL("../apps/web/src/lib/learning-status-stream.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 } }).outputText;
const { LearningStatusParser, observeLearningStatus, StatusHttpError } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`);
const request = "00000000-0000-4000-8000-000000000123";
function frame(sequence, detail, overrides = {}) {
  const event = "reason" in detail ? "closed" : "status";
  const data = { protocol_version: "learning-status-v1", request_id: request, sequence: String(sequence), detail, ...overrides };
  return `event: ${event}\nid: ${sequence}\ndata: ${JSON.stringify(data)}\n\n`;
}
const running = { status: "running", terminal: false };
const succeeded = { status: "succeeded", terminal: true };
test("split frames and heartbeats preserve exact status order", () => {
  const parser = new LearningStatusParser(request), events = [];
  const input = ": 心跳\n\n" + frame(0, running) + frame(1, succeeded);
  for (const char of input) events.push(...parser.push(char));
  parser.finish();
  assert.deepEqual(events, [{ type: "status", ...running }, { type: "status", ...succeeded }]);
});
test("foreign requests, versions, duplicate/gapped sequences and injected fields fail closed", () => {
  for (const invalid of [frame(0, running, { request_id: "foreign" }), frame(0, running, { protocol_version: "v2" }), frame(1, running), frame(0, running, { advice: "private" }), frame(0, { ...running, terminal: true }), frame(0, { status: "other", terminal: true }), frame(0, { reason: "secret" })]) {
    assert.throws(() => new LearningStatusParser(request).push(invalid));
  }
  const parser = new LearningStatusParser(request); parser.push(frame(0, running));
  assert.throws(() => parser.push(frame(0, running)));
});
test("truncated bodies and events after terminal never complete", () => {
  for (const content of [frame(0, running), frame(0, succeeded).slice(0, -1)]) {
    const parser = new LearningStatusParser(request); parser.push(content); assert.throws(() => parser.finish());
  }
  const parser = new LearningStatusParser(request); parser.push(frame(0, succeeded));
  assert.throws(() => parser.push(frame(1, running)));
});
test("frame and total stream sizes are bounded including comments", () => {
  assert.throws(() => new LearningStatusParser(request).push(":" + "x".repeat(2049)));
  const parser = new LearningStatusParser(request);
  assert.throws(() => { for (let i = 0; i < 100; i++) parser.push(":" + "x".repeat(1000) + "\n\n"); });
});
test("all close reasons are explicit terminal observations", () => {
  for (const reason of ["session_unavailable", "request_unavailable", "observation_unavailable", "observation_timeout"]) {
    const parser = new LearningStatusParser(request);
    assert.deepEqual(parser.push(frame(0, { reason })), [{ type: "closed", reason }]); parser.finish();
  }
});
test("stream fetch handles split UTF-8, requires EOF, never reconnects, and cancels its reader", async () => {
  const original = globalThis.fetch; let calls = 0;
  try {
    globalThis.fetch = async (_url, options) => {
      calls++; assert.equal(options.cache, "no-store"); assert.equal(options.method, undefined);
      const bytes = new TextEncoder().encode(": 中文\n\n" + frame(0, running) + frame(1, succeeded));
      return new Response(new ReadableStream({ start(c) { for (const byte of bytes) c.enqueue(Uint8Array.of(byte)); c.close(); } }), { headers: { "content-type": "text/event-stream" } });
    };
    const events = []; await observeLearningStatus(request, new AbortController().signal, e => events.push(e));
    assert.equal(calls, 1); assert.equal(events.length, 2);
    for (const response of [new Response("{}", { headers: { "content-type": "application/json" } }), new Response(frame(0, running), { headers: { "content-type": "text/event-stream" } }), new Response(new Uint8Array(32769), { headers: { "content-type": "text/event-stream" } }), new Response(Uint8Array.of(255), { headers: { "content-type": "text/event-stream" } })]) {
      globalThis.fetch = async () => response;
      await assert.rejects(observeLearningStatus(request, new AbortController().signal, () => {}));
    }
    globalThis.fetch = async () => new Response("{}", { status: 401 });
    await assert.rejects(observeLearningStatus(request, new AbortController().signal, () => {}), e => e instanceof StatusHttpError && e.status === 401);
    let cancelled = false; const controller = new AbortController();
    globalThis.fetch = async () => new Response(new ReadableStream({ start(c) { c.enqueue(new TextEncoder().encode(frame(0, running))); }, cancel() { cancelled = true; } }), { headers: { "content-type": "text/event-stream" } });
    await observeLearningStatus(request, controller.signal, () => controller.abort());
    assert.equal(cancelled, true);
  } finally { globalThis.fetch = original; }
});
