import { test } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { admissionArguments, admit } from "./ollama-answer-admission-lib.mjs";
import { digest, localMetadata } from "./ollama-answer-benchmark-lib.mjs";

const GiB = 1024 ** 3;
const target = { endpoint: "http://127.0.0.1:11434", model_alias: "qwen3.5:9b" };
const modelDigest = "a".repeat(64);
function metadata() {
  return {
    "/api/version": { version: "0.40.7" },
    "/api/tags": { models: [{ name: target.model_alias, model: target.model_alias, digest: modelDigest,
      size: 6 * GiB, details: { format: "gguf", runner: "llamacpp" } }] },
    "/api/show": { capabilities: ["completion", "thinking"], thinking: { values: [false, true] },
      parameters: "temperature 1", details: { runner: "llamacpp" },
      manifests: [{ runner: "llamacpp", selected: true, digest: `sha256:${modelDigest}` }] },
    "/api/ps": { models: [] },
  };
}
function resident(overrides = {}) {
  return { models: [{ name: target.model_alias, model: target.model_alias, digest: modelDigest,
    details: { format: "gguf", runner: "llamacpp" }, runner: "llamacpp", size: 6 * GiB,
    size_vram: 6 * GiB, context_length: 8192, expires_at: new Date(Date.now() + 60000).toISOString(), ...overrides }] };
}
function fixture({ state = { models: [] }, mutate = () => {}, pressure = 80 } = {}) {
  const calls = [];
  let round = 0;
  const dependencies = {
    platform: "darwin", arch: "arm64", benchmarkActive: async () => false,
    totalMemory: async () => String(32 * GiB),
    memoryPressure: async () => `System-wide memory free percentage: ${pressure}%\n`,
    query: async (endpoint, path, body) => {
      calls.push({ endpoint, path, body });
      if (path === "/api/version") round++;
      const values = metadata(); values["/api/ps"] = structuredClone(state);
      mutate(values, round);
      assert.ok(Object.hasOwn(values, path));
      return values[path];
    },
  };
  return { dependencies, calls };
}

test("admission accepts only fixed metadata arguments and canonical local targets", () => {
  for (const command of ["answer-admit", "answer-monitor", "inspect"]) {
    assert.deepEqual(admissionArguments([command, target.endpoint, target.model_alias]), { command, target });
  }
  for (const args of [
    [], ["answer-admit", target.endpoint], ["answer-admit", target.endpoint, target.model_alias, "b".repeat(64)],
    ["run", target.endpoint, target.model_alias], ["answer-admit", "http://localhost:11434", target.model_alias],
    ["answer-admit", "https://127.0.0.1:11434", target.model_alias],
    ["answer-admit", "http://127.0.0.1:11434/api/chat", target.model_alias],
    ["answer-admit", "http://user:secret@127.0.0.1:11434", target.model_alias],
    ["answer-admit", target.endpoint, "remote:cloud"],
  ]) assert.throws(() => admissionArguments(args));
});

test("admission observes metadata twice and emits identity independent of residency and quality profiles", async () => {
  const cold = fixture(), warm = fixture({ state: resident() });
  const admitted = await admit(target, cold.dependencies);
  const reused = await admit(target, warm.dependencies);
  assert.match(admitted.identity_sha256, /^[a-f0-9]{64}$/);
  assert.equal(admitted.identity_sha256, reused.identity_sha256);
  const { identity_sha256, residency, total_memory, minimum_available_bytes, ...identity } = admitted;
  assert.equal(identity_sha256, digest(identity));
  assert.equal(Object.hasOwn(identity, "execution_profile"), false);
  assert.equal(residency, "cold"); assert.equal(reused.residency, "reuse");
  assert.equal(total_memory, 32 * GiB); assert.ok(minimum_available_bytes > 16 * GiB);
  assert.equal(cold.calls.length, 8);
  for (const call of cold.calls) {
    assert.equal(call.endpoint, target.endpoint);
    assert.deepEqual(call.body, call.path === "/api/show" ? { model: target.model_alias, runner: "llamacpp" } : null);
    assert.ok(["/api/version", "/api/tags", "/api/show", "/api/ps"].includes(call.path));
  }
});

test("cold and reload require loading budget while reuse keeps one GiB headroom", async () => {
  const warm = await admit(target, fixture({ state: resident(), pressure: 30 }).dependencies);
  assert.equal(warm.residency, "reuse");
  for (const state of [{ models: [] }, resident({ context_length: 4096 }), resident({ expires_at: new Date().toISOString() })]) {
    await assert.rejects(admit(target, fixture({ state, pressure: 30 }).dependencies), error => error.answerFailure === "resource");
  }
  await assert.rejects(admit(target, fixture({ state: resident(), pressure: 23 }).dependencies), error => error.answerFailure === "resource");
});

test("inflight monitor keeps only reserve and tolerates own cold to resident transition", async () => {
  const transition = { pressure: 21, mutate: (values, round) => { if (round === 2) values["/api/ps"] = resident(); } };
  const observed = await admit(target, fixture(transition).dependencies, "answer-monitor");
  assert.equal(observed.residency, "reuse");
  await assert.rejects(admit(target, fixture({ ...transition, pressure: 80 }).dependencies), /residency changed/);
  await assert.rejects(admit(target, fixture({ state: resident(), pressure: 21 }).dependencies), error => error.answerFailure === "resource");
  await assert.rejects(admit(target, fixture({ state: resident(), pressure: 20 }).dependencies, "answer-monitor"), error => error.answerFailure === "resource");
});

test("runtime version, digest and metadata changes stop both guards", async () => {
  for (const command of ["answer-admit", "answer-monitor"]) {
    for (const change of [
      values => values["/api/version"].version = "0.40.8",
      values => values["/api/show"].parameters = "temperature 0",
      values => {
        values["/api/tags"].models[0].digest = "b".repeat(64);
        values["/api/show"].manifests[0].digest = `sha256:${"b".repeat(64)}`;
      },
      values => values["/api/tags"].models[0].size++,
    ]) {
      await assert.rejects(admit(target, fixture({ mutate: (values, round) => { if (round === 2) change(values); } }).dependencies, command), /runtime/);
    }
  }
});

test("cloud metadata, unsupported identity and other resident models never grant admission", async () => {
  for (const command of ["answer-admit", "answer-monitor"]) {
    for (const change of [
      values => values["/api/tags"].models[0].remote_host = "https://example.invalid",
      values => values["/api/show"].remote_model = "remote",
      values => values["/api/show"].thinking.values = [true],
      values => values["/api/version"].version = "0.39.0",
      values => values["/api/show"].manifests.push(values["/api/show"].manifests[0]),
      values => values["/api/ps"] = resident({ name: "other" }),
      values => values["/api/ps"] = { models: [resident().models[0], resident().models[0]] },
      values => values["/api/ps"] = resident({ remote_host: "https://example.invalid" }),
      values => values["/api/ps"] = resident({ runner: "other" }),
    ]) await assert.rejects(admit(target, fixture({ mutate: change }).dependencies, command));
  }
});

test("existing benchmark lock rejects before observation and if it appears during observation", async () => {
  for (const command of ["answer-admit", "answer-monitor"]) {
    const active = fixture(); active.dependencies.benchmarkActive = async () => true;
    await assert.rejects(admit(target, active.dependencies, command), /benchmark is active/);
    assert.equal(active.calls.length, 0);
    const later = fixture(); let checks = 0;
    later.dependencies.benchmarkActive = async () => ++checks > 1;
    await assert.rejects(admit(target, later.dependencies, command), /benchmark is active/);
    assert.equal(checks, 2);
    const unavailable = fixture(); unavailable.dependencies.benchmarkActive = async () => { throw new Error("cannot observe lock"); };
    await assert.rejects(admit(target, unavailable.dependencies, command), /cannot observe lock/);
    assert.equal(unavailable.calls.length, 0);
  }
});

test("cancellation, memory query failure and final budget failure are terminal observations", async () => {
  const stopped = fixture(), abort = new AbortController(); abort.abort(); stopped.dependencies.signal = abort.signal;
  await assert.rejects(admit(target, stopped.dependencies), /cancelled/); assert.equal(stopped.calls.length, 0);
  for (const change of [
    dependencies => dependencies.platform = "linux", dependencies => dependencies.arch = "x64",
    dependencies => dependencies.totalMemory = async () => "unknown",
    dependencies => dependencies.memoryPressure = async () => "unavailable",
    dependencies => dependencies.memoryPressure = async () => { throw new Error("pressure unavailable"); },
    dependencies => { let count = 0; dependencies.memoryPressure = async () => `System-wide memory free percentage: ${++count === 1 ? 80 : 1}%`; },
    dependencies => {
      const controller = new AbortController(); dependencies.signal = controller.signal;
      dependencies.memoryPressure = async () => { controller.abort(); return "System-wide memory free percentage: 80%"; };
    },
  ]) { const failed = fixture(); change(failed.dependencies); await assert.rejects(admit(target, failed.dependencies)); }
});

test("local HTTP fixture sees only bounded metadata requests and no private request material", async () => {
  const calls = [], values = metadata();
  const server = http.createServer(async (request, response) => {
    let body = ""; for await (const chunk of request) body += chunk;
    calls.push({ path: request.url, method: request.method, body, authorization: request.headers.authorization });
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify(values[request.url]));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  try {
    const endpoint = `http://127.0.0.1:${server.address().port}`;
    const dependencies = { ...fixture().dependencies, query: localMetadata, signal: AbortSignal.timeout(1000) };
    const result = await admit({ ...target, endpoint }, dependencies);
    assert.match(result.identity_sha256, /^[a-f0-9]{64}$/); assert.equal(calls.length, 8);
    for (const call of calls) {
      assert.equal(call.authorization, undefined); assert.ok(Object.hasOwn(values, call.path));
      assert.equal(call.method, call.path === "/api/show" ? "POST" : "GET");
      assert.equal(call.body, call.path === "/api/show" ? JSON.stringify({ model: target.model_alias, runner: "llamacpp" }) : "");
    }
    values["/api/show"].remote_host = "https://example.invalid";
    await assert.rejects(admit({ ...target, endpoint }, dependencies));
    // The first invalid identity may reject while the parallel ps observation is pending.
    const rejected = calls.slice(8);
    assert.ok(rejected.length >= 3 && rejected.length <= 4);
    assert.equal(new Set(rejected.map(call => call.path)).size, rejected.length);
  } finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
});

test("CLI rejects unexpected arguments without echoing them or producing stdout", async () => {
  const execute = promisify(execFile);
  await assert.rejects(execute(process.execPath, [new URL("./ollama-answer-admission.mjs", import.meta.url).pathname,
    "answer-admit", target.endpoint, target.model_alias, "private-secret-never-echo"], { timeout: 1000 }), error => {
    assert.equal(error.code, 1); assert.equal(error.stdout, "");
    assert.equal(error.stderr.includes("private-secret-never-echo"), false);
    assert.match(error.stderr, /Ollama 问答准入未确认/); return true;
  });
});
