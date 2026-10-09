import { test } from "node:test";
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import http from "node:http";
import { availableMemory, guardedSend, localMetadata, memoryBudget, memoryReady, parseOptions, qualityGate, runtimeIdentity, PROFILE, REPORT_SCHEMA, SUITES, RESERVE_BYTES, RESOURCE_POLICY } from "./ollama-answer-benchmark-lib.mjs";
const target = { endpoint: "http://127.0.0.1:11434", model_alias: "qwen3.5:9b" };
const version = { version: "0.40.1" };
const tags = { models: [{ name: target.model_alias, model: target.model_alias, digest: "a".repeat(64), size: 1000, details: { format: "gguf", runner: "llamacpp" } }] };
const show = { capabilities: ["completion", "thinking"], thinking: { values: [false, true] }, parameters: "temperature 1", details: { runner: "llamacpp" }, manifests: [{ runner: "llamacpp", selected: true, digest: `sha256:${"a".repeat(64)}` }] };
function runtime() { return runtimeIdentity(target, version, tags, show); }
test("local metadata rejects redirects, invalid JSON, overflow and continuous responses past the deadline", async () => {
  let mode = "good", calls = 0;
  const server = http.createServer((request, response) => {
    calls++; assert.equal(request.headers.authorization, undefined);
    if (mode === "redirect") { response.writeHead(302, { location: "/api/version" }); response.end(); }
    else if (mode === "overflow") response.end("x".repeat(1024 * 1024 + 1));
    else if (mode === "invalid") response.end("secret model text");
    else if (mode === "continuous") {
      response.writeHead(200); response.write("{");
      const timer = setInterval(() => response.write(" "), 5);
      response.on("close", () => clearInterval(timer));
    } else response.end(JSON.stringify(version));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const endpoint = `http://127.0.0.1:${server.address().port}`;
  try {
    assert.deepEqual(await localMetadata(endpoint, "/api/version"), version);
    for (const next of ["redirect", "overflow", "invalid", "continuous"]) {
      mode = next;
      await assert.rejects(localMetadata(endpoint, "/api/version"));
    }
    assert.equal(calls, 5);
  } finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
});
test("only exact local installed models, supported thinking control and canonical loopback are accepted", () => {
  assert.equal(runtime().execution_profile, PROFILE);
  assert.equal(parseOptions(["--suite", "challenge"]).model, "qwen3.5:9b");
  assert.equal(parseOptions(["--suite", "coverage"]).suite, "coverage");
  assert.equal(parseOptions(["--suite", "extraction"]).suite, "extraction");
  for (const options of [
    ["--endpoint", "https://example.com"], ["--endpoint", "http://localhost:11434"],
    ["--endpoint", "http://127.0.0.1:11434/path"], ["--endpoint", "http://127.0.0.1:011434"],
    ["--endpoint", "http://127.0.0.1:65536"], ["--model", "remote:cloud"], ["--model", "remote-cloud"],
    ["--model", "m", "--model", "m"], ["--suite", "later"], ["--model"], ["--secret", "x"],
  ]) assert.throws(() => parseOptions(options));
  for (const change of [
    (t, s) => t.models[0].remote_model = "cloud", (t, s) => s.remote_host = "https://example.com",
    (t, s) => s.thinking.values = [true], (t, s) => s.capabilities = ["vision"],
    (t, s) => t.models[0].size = 0, (t, s) => t.models[0].digest = "changed",
    (t, s) => t.models[0].name = "other", (t, s) => t.models.push(t.models[0]),
    (t, s) => s.manifests[0].digest = "b".repeat(64), (t, s) => s.details.runner = "ggml",
  ]) {
    const t = structuredClone(tags), s = structuredClone(show); change(t, s);
    assert.throws(() => runtimeIdentity(target, version, t, s));
  }
  assert.notDeepEqual(runtime(), runtimeIdentity(target, version, tags, { ...show, parameters: "temperature 0" }));
  const variants = structuredClone(tags); variants.models.push({ ...variants.models[0], details: { format: "gguf", runner: "ggml" } });
  assert.deepEqual(runtime(), runtimeIdentity(target, version, variants, show));
  assert.throws(() => runtimeIdentity(target, { version: "0.39.0" }, tags, show));
});
test("unified memory guard rejects missing queries and accounts for loading plus reserve", () => {
  assert.equal(availableMemory(10000, "header\nSystem-wide memory free percentage: 61%\n"), 6100);
  for (const value of ["", "System-wide memory free percentage: 101%", "System-wide memory free percentage: 61%\nSystem-wide memory free percentage: 61%", "System-wide memory free percentage: unknown%"]) assert.throws(() => availableMemory(10000, value));
  assert.equal(memoryBudget(RESERVE_BYTES + 1500, 1000, true), RESERVE_BYTES + 1500);
  assert.throws(() => memoryBudget(RESERVE_BYTES + 1499, 1000, true));
  assert.throws(() => memoryBudget(RESERVE_BYTES - 1, 1000, false));
  assert.equal(memoryBudget(RESERVE_BYTES, 1000, false), RESERVE_BYTES);
});
test("pressure query failure cancels the one pending request and never resends", async () => {
  let calls = 0, cancelled = false;
  await assert.rejects(guardedSend(signal => {
    calls++;
    return new Promise((_, reject) => signal.addEventListener("abort", () => { cancelled = true; reject(new Error("aborted")); }, { once: true }));
  }, async () => { throw new Error("query unavailable"); }, new AbortController().signal, 1), error => error.answerFailure === "resource");
  assert.equal(calls, 1); assert.equal(cancelled, true);
});
test("memory settling only rereads pressure within a deadline and never hides query failures", async () => {
  let samples = 0;
  const ready = await memoryReady(async () => ++samples < 2 ? RESERVE_BYTES : RESERVE_BYTES + 1500, 1000, undefined, 30, 1);
  assert.equal(samples, 2); assert.equal(ready.available, RESERVE_BYTES + 1500);
  await assert.rejects(memoryReady(async () => RESERVE_BYTES, 1000, undefined, 2, 1), error => error.answerFailure === "resource");
  samples = 0;
  await assert.rejects(memoryReady(async () => { samples++; throw new Error("query failed"); }, 1000));
  assert.equal(samples, 1);
});
test("monitor stops before the final pressure confirmation so a late sample cannot be ignored", async () => {
  let finish, samples = 0;
  const sent = new Promise(resolve => { finish = resolve; });
  assert.equal(await guardedSend(() => sent, async () => {
    samples++;
    if (samples === 1) finish("completed");
    else await delay(5);
  }, undefined, 1), "completed");
  assert.equal(samples, 2);
  await assert.rejects(guardedSend(async () => "completed", async () => { throw new Error("final pressure unavailable"); }), error => error.answerFailure === "resource");
});
function fixtures() {
  const expected = Object.fromEntries(Object.entries(SUITES).map(([suite, version]) => [suite, [{ execution_profile: PROFILE, request_sha256: "b".repeat(64), case: { id: suite, suite: version, synthetic_only: true } }]]));
  const reports = Object.keys(SUITES).flatMap(suite => [suite, suite]).map((suite, index) => ({
    schema: REPORT_SCHEMA, run_id: `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`, synthetic_only: true, suite,
    runtime: { ...runtime(), platform: "darwin", arch: "arm64", resource_policy: RESOURCE_POLICY, total_memory: 32 * 1024 ** 3 }, started_at: new Date(10000 + index * 10000).toISOString(), ended_at: new Date(11000 + index * 10000).toISOString(),
    resources: { preload_checks: 1, inflight_checks: 1, minimum_preload_available_bytes: RESERVE_BYTES + 1500, minimum_inflight_available_bytes: RESERVE_BYTES, settle_wait_ms: 0 },
    manifests: structuredClone(expected[suite]), total_cases: 1, passed_cases: 1, complete: true, not_run: [], exit_code: 0,
    results: [{ id: suite, request_sha256: "b".repeat(64), elapsed_ms: 1, protocol_valid: true, citation_valid: true, quality_pass: true,
      checks: ["expected_status", "required_terms", "expected_citations", "forbidden_terms"].map(name => ({ name, passed: true })) }],
  }));
  return { expected, reports };
}
test("two rounds of each unchanged suite are required; failures stay failures and grant no consent", () => {
  const { expected, reports } = fixtures();
  const passed = qualityGate(reports, expected);
  assert.equal(passed.passed, true); assert.equal(passed.private_execution_authorized, false);
  assert.equal(passed.schema, "ollama-answer-quality-gate-v3");
  assert.equal(passed.evaluated_cases, 8);
  const failure = reports[5]; failure.results[0].checks[1].passed = false; failure.results[0].quality_pass = false; failure.passed_cases = 0; failure.exit_code = 2;
  const failed = qualityGate(reports, expected);
  assert.equal(failed.passed, false); assert.equal(failed.failed_cases.length, 1);
});
test("duplicate, incomplete, overlapping, reordered, relabelled and different runtime reports cannot pass", () => {
  for (const mutate of [
    reports => reports.pop(), reports => reports[1] = structuredClone(reports[0]),
    reports => reports.splice(4), reports => reports[5].suite = "challenge",
    reports => reports.forEach(report => report.runtime.execution_profile = "ollama-knowledge-answer-v3"),
    reports => reports.forEach(report => report.runtime.execution_profile = "ollama-knowledge-answer-v4"),
    reports => reports.forEach(report => report.runtime.execution_profile = "ollama-knowledge-answer-v5"),
    reports => reports.forEach(report => report.runtime.execution_profile = "ollama-knowledge-answer-v6"),
    reports => reports.splice(6), reports => reports[7].suite = "coverage",
    reports => reports[1].runtime.version = "0.40.2", reports => reports[1].runtime.model_digest = "c".repeat(64),
    reports => reports[1].runtime.model_metadata_sha256 = "d".repeat(64),
    reports => reports[1].manifests[0].request_sha256 = "e".repeat(64),
    reports => reports[1].manifests[0].case.required_terms = ["lowered"],
    reports => reports[1].results[0].request_sha256 = "e".repeat(64),
    reports => reports[1].results[0].answer = "unexpected model text",
    reports => reports[1].started_at = reports[0].started_at,
    reports => reports[1].complete = false, reports => reports[1].not_run = ["missing"],
    reports => reports[1].exit_code = 2, reports => reports[1].failure = "protocol",
    reports => reports[1].results[0].protocol_valid = false,
    reports => reports[1].results[0].checks.reverse(),
    reports => reports[1].resources.preload_checks = 0,
    reports => reports[1].resources.minimum_inflight_available_bytes = RESERVE_BYTES - 1,
    reports => reports[1].answer = "unexpected model text",
    reports => reports[1].failure_stage = "selection_quote",
    reports => reports[1].diagnostic_only = true,
    reports => reports.forEach(report => report.runtime.version = "0.39.0"),
    reports => reports.forEach(report => report.runtime.resource_policy = "unprotected"),
  ]) {
    const { reports, expected } = fixtures(); mutate(reports);
    assert.throws(() => qualityGate(reports, expected));
  }
  const { reports, expected } = fixtures(); delete expected.coverage;
  assert.throws(() => qualityGate(reports, expected));
});
