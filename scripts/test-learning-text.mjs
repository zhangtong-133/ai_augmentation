import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
const require = createRequire(new URL("../apps/web/package.json", import.meta.url));
const ts = require("typescript");
async function compiled(path) {
  return ts.transpileModule(await readFile(new URL(path, import.meta.url), "utf8"), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 } }).outputText;
}
const url = source => `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`;
const statusUrl = url(await compiled("../apps/web/src/lib/learning-status-stream.ts"));
const source = (await compiled("../apps/web/src/lib/learning-text-stream.ts")).replaceAll('"./learning-status-stream"', JSON.stringify(statusUrl));
const { LearningTextParser, observeLearningText } = await import(url(source));
const request = "00000000-0000-4000-8000-000000000123";
function frame(sequence, event, detail, overrides = {}) {
  return `event: ${event}\nid: ${sequence}\ndata: ${JSON.stringify({ protocol_version: "learning-text-v1", request_id: request, sequence: String(sequence), detail, ...overrides })}\n\n`;
}
const running = { status: "running", terminal: false }, succeeded = { status: "succeeded", terminal: true };
test("live text remains temporary through splitting, clear and persisted terminal", () => {
  const parser = new LearningTextParser(request), events = [];
  const input = ": 中文\n\n" + frame(0, "status", running) + frame(1, "delta", { text: "未校验 <script> 中文" }) + frame(2, "clear", {}) + frame(3, "status", succeeded);
  for (const char of input) events.push(...parser.push(char));
  parser.finish();
  assert.deepEqual(events, [{ type: "status", ...running }, { type: "delta", text: "未校验 <script> 中文" }, { type: "clear" }, { type: "status", ...succeeded }]);
});
test("foreign binding, unknown fields, bad sequence and protocol fail permanently", () => {
  for (const invalid of [frame(0,"status",running,{request_id:"foreign"}),frame(0,"status",running,{protocol_version:"other"}),frame(1,"status",running),frame(0,"status",running,{advice:true}),frame(0,"status",{...running,terminal:true}),frame(0,"delta",{text:"no initial status"}),frame(0,"closed",{reason:"secret"})]) {
    const parser = new LearningTextParser(request); assert.throws(() => parser.push(invalid));
    assert.throws(() => parser.push(frame(0,"status",succeeded))); assert.throws(() => parser.finish());
  }
  for (const invalid of [frame(1,"delta",{text:"x",score:3}),frame(0,"delta",{text:"duplicate"}),frame(2,"delta",{text:"gap"}),frame(1,"other",{}),frame(1,"delta",{text:""})]) {
    const parser = new LearningTextParser(request); parser.push(frame(0,"status",running)); assert.throws(() => parser.push(invalid));
  }
});
test("text after clear, extra terminals and incomplete streams cannot succeed", () => {
  const parser = new LearningTextParser(request); parser.push(frame(0,"status",running)+frame(1,"clear",{}));
  assert.throws(() => parser.push(frame(2,"delta",{text:"late"})));
  for (const body of [frame(0,"status",running),frame(0,"status",succeeded).slice(0,-1),frame(0,"status",succeeded)+frame(1,"clear",{})]) {
    const parser = new LearningTextParser(request); assert.throws(() => { parser.push(body); parser.finish(); });
  }
});
test("UTF-8 text, frame, packet and overall limits bound temporary memory", () => {
  for (const text of ["中".repeat(171),"x".repeat(513)]) {
    const parser = new LearningTextParser(request); parser.push(frame(0,"status",running)); assert.throws(() => parser.push(frame(1,"delta",{text})));
  }
  const parser = new LearningTextParser(request); parser.push(frame(0,"status",running));
  for (let i=1;i<=48;i++) parser.push(frame(i,"delta",{text:"x".repeat(512)}));
  assert.throws(() => parser.push(frame(49,"delta",{text:"x"})));
  assert.throws(() => new LearningTextParser(request).push(":"+"x".repeat(4096)));
  const oversized = new LearningTextParser(request);
  assert.throws(() => { for (let i=0;i<150;i++) oversized.push(":"+"x".repeat(4000)+"\n\n"); });
  const many = new LearningTextParser(request);
  assert.throws(() => { for(let i=0;i<=1100;i++) many.push(frame(i,"status",running)); });
});
test("fetch consumes split UTF-8 once and always cancels its stream", async () => {
  const original = globalThis.fetch; let calls = 0;
  try {
    globalThis.fetch = async (path, options) => {
      calls++; assert.match(path,/\/text-events$/); assert.equal(options.cache,"no-store");
      const bytes = new TextEncoder().encode(frame(0,"status",running)+frame(1,"delta",{text:"中文"})+frame(2,"status",succeeded));
      return new Response(new ReadableStream({ start(c) { for(const byte of bytes) c.enqueue(Uint8Array.of(byte)); c.close(); } }),{ headers:{"content-type":"text/event-stream"} });
    };
    const events=[]; await observeLearningText(request,new AbortController().signal,e=>events.push(e));
    assert.equal(calls,1); assert.equal(events[1].text,"中文");
    for (const response of [new Response("{}",{headers:{"content-type":"application/json"}}),new Response(Uint8Array.of(255),{headers:{"content-type":"text/event-stream"}}),new Response(frame(0,"status",running),{headers:{"content-type":"text/event-stream"}}),new Response(new Uint8Array(524289),{headers:{"content-type":"text/event-stream"}})]) {
      globalThis.fetch=async()=>response; await assert.rejects(observeLearningText(request,new AbortController().signal,()=>{}));
    }
    globalThis.fetch=async()=>new Response("{}",{status:503});
    await assert.rejects(observeLearningText(request,new AbortController().signal,()=>{}),e=>e.status===503);
    let cancelled=false; const controller=new AbortController();
    globalThis.fetch=async()=>new Response(new ReadableStream({start(c){c.enqueue(new TextEncoder().encode(frame(0,"status",running)));},cancel(){cancelled=true;}}),{headers:{"content-type":"text/event-stream"}});
    await observeLearningText(request,controller.signal,()=>controller.abort()); assert.equal(cancelled,true);
  } finally { globalThis.fetch=original; }
});
