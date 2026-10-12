import { test } from "node:test";
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import http from "node:http";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { stagedCli } from "./ollama-answer-stages.mjs";
import { confirmResidentRequest, residentState } from "./ollama-answer-residency.mjs";
import { availableMemory, guardedSend, localMetadata, memoryBudget, memoryReady, parseOptions, qualityGate, residencyPolicy, runtimeIdentity, PROFILE, REUSE_PROFILE, REPORT_SCHEMA, SUITES, RESERVE_BYTES, RESOURCE_POLICY, REUSE_HEADROOM_BYTES } from "./ollama-answer-benchmark-lib.mjs";
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
  assert.equal(parseOptions([]).residency, "reuse");
  assert.equal(parseOptions(["--residency", "cold"]).residency, "cold");
  assert.equal(runtimeIdentity({ ...target, residency: "reuse" }, version, tags, show).execution_profile, REUSE_PROFILE);
  assert.equal(parseOptions(["--suite", "challenge"]).model, "qwen3.5:9b");
  assert.equal(parseOptions(["--suite", "coverage"]).suite, "coverage");
  assert.equal(parseOptions(["--suite", "extraction"]).suite, "extraction");
  assert.equal(parseOptions(["--suite", "decision"]).suite, "decision");
  assert.equal(parseOptions(["--suite", "mixed"]).suite, "mixed");
  assert.equal(parseOptions(["--suite", "availability"]).suite, "availability");
  assert.equal(parseOptions(["--suite", "support"]).suite, "support");
  for (const options of [
    ["--endpoint", "https://example.com"], ["--endpoint", "http://localhost:11434"],
    ["--endpoint", "http://127.0.0.1:11434/path"], ["--endpoint", "http://127.0.0.1:011434"],
    ["--endpoint", "http://127.0.0.1:65536"], ["--model", "remote:cloud"], ["--model", "remote-cloud"],
    ["--model", "m", "--model", "m"], ["--suite", "later"], ["--model"], ["--secret", "x"],
    ["--residency", "forever"], ["--residency", "reuse", "--residency", "cold"],
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
function fixtures(residency = "cold") {
  const policy = residencyPolicy(residency);
  const expected = Object.fromEntries(Object.entries(SUITES).map(([suite, version]) => [suite, [{ execution_profile: policy.profile, request_sha256: "b".repeat(64), case: { id: suite, suite: version, synthetic_only: true } }]]));
  const reports = Object.keys(SUITES).flatMap(suite => [suite, suite]).map((suite, index) => ({
    schema: policy.report_schema, run_id: `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`, synthetic_only: true, suite,
    runtime: { ...runtimeIdentity({ ...target, residency }, version, tags, show), platform: "darwin", arch: "arm64", resource_policy: policy.resource_policy, total_memory: 32 * 1024 ** 3 }, started_at: new Date(10000 + index * 10000).toISOString(), ended_at: new Date(11000 + index * 10000).toISOString(),
    resources: { preload_checks: 2, inflight_checks: 1, minimum_preload_available_bytes: RESERVE_BYTES + 1500, minimum_inflight_available_bytes: RESERVE_BYTES, settle_wait_ms: 0,
      ...(residency === "reuse" ? { cold_checks: 1, reload_checks: 0, reuse_checks: 1, minimum_loading_available_bytes: RESERVE_BYTES + 1500, minimum_reuse_available_bytes: RESERVE_BYTES + REUSE_HEADROOM_BYTES } : {}) },
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
  assert.equal(passed.schema, "ollama-answer-quality-gate-v8");
  assert.equal(passed.evaluated_cases, 16);
  assert.equal(passed.native_requests_per_case, 2);
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
    reports => reports.forEach(report => report.runtime.execution_profile = "ollama-knowledge-answer-v7"),
    reports => reports.forEach(report => report.runtime.execution_profile = "ollama-knowledge-answer-v8"),
    reports => reports.forEach(report => report.runtime.execution_profile = "ollama-knowledge-answer-v9"),
    reports => reports.forEach(report => report.runtime.execution_profile = "ollama-knowledge-answer-v10"),
    reports => reports.forEach(report => report.runtime.execution_profile = "ollama-knowledge-answer-v11"),
    reports => reports.forEach(report => report.runtime.execution_profile = "ollama-knowledge-answer-v12"),
    reports => reports.forEach(report => report.runtime.execution_profile = "ollama-knowledge-answer-v13"),
    reports => reports[1].schema = "ollama-answer-quality-report-v1",
    reports => reports[1].resources.preload_checks = 1,
    reports => reports.splice(6), reports => reports[7].suite = "coverage",
    reports => reports.splice(8), reports => reports[9].suite = "extraction",
    reports => reports.splice(10), reports => reports[11].suite = "decision",
    reports => reports.splice(12), reports => reports[13].suite = "mixed",
    reports => reports.splice(14), reports => reports[15].suite = "availability",
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
  const noMixed = fixtures(); delete noMixed.expected.mixed;
  assert.throws(() => qualityGate(noMixed.reports, noMixed.expected));
  const noAvailability = fixtures(); delete noAvailability.expected.availability;
  assert.throws(() => qualityGate(noAvailability.reports, noAvailability.expected));
  const noSupport = fixtures(); delete noSupport.expected.support;
  assert.throws(() => qualityGate(noSupport.reports, noSupport.expected));
});

function loaded(overrides = {}) {
  return { models: [{ name: target.model_alias, model: target.model_alias, digest: runtime().model_digest,
    details: { runner: "llamacpp", format: "gguf" }, size: 3000, size_vram: 2500, context_length: 8192,
    expires_at: new Date(Date.now() + 30000).toISOString(), ...overrides }] };
}
test("only the same resident model, runner and context with a live lease can use the warm budget", () => {
  assert.deepEqual(residentState(runtime(), { models: [] }), { state: "cold", loading: true });
  assert.deepEqual(residentState(runtime(), loaded()), { state: "reuse", loading: false });
  assert.deepEqual(residentState(runtime(), loaded({ runner: "llamacpp", details: { format: "gguf" } })), { state: "reuse", loading: false });
  for (const state of [loaded({ context_length: 4096 }), loaded({ expires_at: new Date().toISOString() }), loaded({ details: { format: "gguf" } })]) {
    assert.deepEqual(residentState(runtime(), state), { state: "reload", loading: true });
  }
  for (const state of [
    {}, { models: [loaded().models[0], loaded().models[0]] },
    loaded({ name: "other" }), loaded({ model: "other" }), loaded({ digest: "b".repeat(64) }),
    loaded({ details: { format: "gguf", runner: "ggml" } }), loaded({ remote_host: "external" }),
    loaded({ runner: "ggml" }), loaded({ runner: "llamacpp", details: { format: "gguf", runner: "ggml" } }),
    loaded({ size: 0 }), loaded({ size_vram: -1 }), loaded({ context_length: 0 }), loaded({ expires_at: "unknown" }),
  ]) assert.throws(() => residentState(runtime(), state));
});
test("warm requests keep generation headroom and a disappearing or replaced model grants no permission", async () => {
  const model = { ...runtime(), model_size: 4 * 1024 ** 3 }, sufficient = RESERVE_BYTES + REUSE_HEADROOM_BYTES;
  let queries = 0;
  const warm = await confirmResidentRequest(model, async () => { queries++; return loaded(); }, async () => sufficient, undefined, 0, 1);
  assert.equal(warm.state, "reuse"); assert.equal(warm.available, sufficient); assert.equal(queries, 2);
  // The same free memory cannot pay for a cold load, and resident requests need their extra GiB.
  await assert.rejects(confirmResidentRequest(model, async () => ({ models: [] }), async () => sufficient, undefined, 0, 1), issue => issue.answerFailure === "resource");
  await assert.rejects(confirmResidentRequest(model, async () => loaded(), async () => sufficient - 1, undefined, 0, 1), issue => issue.answerFailure === "resource");
  await assert.rejects(confirmResidentRequest(model, async () => loaded(), async () => { throw new Error("query unavailable"); }), issue => issue.answerFailure === "resource");
  for (const next of [{ models: [] }, loaded({ digest: "b".repeat(64) }), loaded({ context_length: 4096 })]) {
    let calls = 0;
    await assert.rejects(confirmResidentRequest(model, async () => ++calls === 1 ? loaded() : next, async () => sufficient), issue => issue.answerFailure === "runtime");
    assert.equal(calls, 2);
  }
  const abort = new AbortController();
  await assert.rejects(confirmResidentRequest(model, async () => loaded(), async () => { abort.abort(); return sufficient; }, abort.signal), issue => issue.answerFailure === "runtime");
});
test("warm reports bind a new request profile and reserve; cold and warm results cannot be combined", () => {
  const { reports, expected } = fixtures("reuse");
  const gate = qualityGate(reports, expected);
  assert.equal(gate.schema, "ollama-answer-quality-gate-v9");
  assert.equal(gate.passed, true); assert.equal(gate.private_execution_authorized, false);
  for (const mutate of [
    reports => reports[1] = fixtures().reports[1],
    reports => reports[1].schema = REPORT_SCHEMA,
    reports => reports[1].resources.reuse_checks = 0,
    reports => reports[1].resources.cold_checks = 0,
    reports => reports[1].resources.minimum_loading_available_bytes = null,
    reports => reports[1].resources.minimum_reuse_available_bytes = RESERVE_BYTES,
    reports => reports[1].resources.minimum_preload_available_bytes++,
    reports => reports.forEach(report => report.runtime.resource_policy = RESOURCE_POLICY),
  ]) {
    const { reports, expected } = fixtures("reuse"); mutate(reports);
    assert.throws(() => qualityGate(reports, expected));
  }
  assert.throws(() => qualityGate(fixtures().reports, expected));
  // Starting with an already resident model needs no invented cold-loading observation.
  for (const report of reports) Object.assign(report.resources, { cold_checks: 0, reuse_checks: 2,
    minimum_loading_available_bytes: null, minimum_preload_available_bytes: report.resources.minimum_reuse_available_bytes });
  assert.equal(qualityGate(reports, expected).passed, true);
});

const childOptions = { env: {} };
function childArgs(stages, path) {
  return ["--input-type=module", "-e", `
    import { createInterface } from 'node:readline';
    import { appendFileSync } from 'node:fs';
    const input = createInterface({input:process.stdin});
    for (const stage of ${JSON.stringify(stages)}) {
      process.stderr.write('ANSWER_NATIVE_STAGE='+stage+'\\n');
      const ack=await new Promise(resolve=>input.once('line',resolve));
      if (ack!=='allow') process.exit(1);
      ${path ? `appendFileSync(${JSON.stringify(path)},stage+'\\n',{mode:0o600});` : ""}
    }
    process.stdout.write(JSON.stringify({completed:true})); input.close(); process.stdin.destroy();
  `];
}
test("each native request waits for its own confirmed host check in strict stage order", async () => {
  const seen=[];
  const raw=await stagedCli(process.execPath,childArgs(["select","verify"]),childOptions,async stage=>{ seen.push(stage); await delay(5); });
  assert.deepEqual(seen,["select","verify"]); assert.deepEqual(raw,{completed:true});
});
test("a failed host check prevents that request and never retries either native stage", async () => {
  const directory=await mkdtemp(join(tmpdir(),"ollama-stage-test-"));
  try {
    for (const blocked of ["select","verify"]) {
      const path=join(directory,blocked), seen=[];
      await assert.rejects(stagedCli(process.execPath,childArgs(["select","verify"],path),childOptions,async stage=>{
        seen.push(stage); if(stage===blocked) throw Object.assign(new Error("reserve unavailable"),{answerFailure:"resource"});
      }),issue=>issue.answerFailure==="resource");
      const sent=await readFile(path,"utf8").catch(issue=>{ if(issue.code!=="ENOENT") throw issue; return ""; });
      assert.equal(sent,blocked==="select"?"":"select\n");
      assert.deepEqual(seen,blocked==="select"?["select"]:["select","verify"]);
    }
  } finally { await rm(directory,{recursive:true,force:true}); }
});
test("native handshake can reuse a warm model but never grants its second request below the warm reserve", async () => {
  const directory = await mkdtemp(join(tmpdir(), "ollama-reuse-test-"));
  const model = { ...runtime(), model_size: 4 * 1024 ** 3 };
  try {
    for (const enough of [true, false]) {
      const path = join(directory, String(enough)), seen = [];
      const probe = stagedCli(process.execPath, childArgs(["select", "verify"], path), childOptions, async (stage, signal) => {
        const warm = stage === "verify";
        const free = warm ? RESERVE_BYTES + REUSE_HEADROOM_BYTES - (enough ? 0 : 1) : RESERVE_BYTES + 6 * 1024 ** 3;
        const ready = await confirmResidentRequest(model, async () => warm ? loaded({ runner: "llamacpp", details: { format: "gguf" } }) : { models: [] }, async () => free, signal, 0, 1);
        seen.push(ready.state);
      });
      if (enough) assert.deepEqual(await probe, { completed: true });
      else await assert.rejects(probe, issue => issue.answerFailure === "resource");
      assert.equal(await readFile(path, "utf8"), enough ? "select\nverify\n" : "select\n");
      assert.deepEqual(seen, enough ? ["cold", "reuse"] : ["cold"]);
    }
  } finally { await rm(directory, { recursive: true, force: true }); }
});
test("missing repeated reordered extra or malformed stage markers cannot count as completed", async () => {
  for (const stages of [[],["select"],["verify"],["select","select"],["select","verify","verify"],["secret"]]) {
    await assert.rejects(stagedCli(process.execPath,childArgs(stages),childOptions,async()=>{}),issue=>issue.answerFailureStage==="native_guard");
  }
});
test("cancellation and deadline stop a waiting native stage before acknowledgment", async () => {
  for (const cancel of [true,false]) {
    const controller=new AbortController(); let checks=0;
    await assert.rejects(stagedCli(process.execPath,childArgs(["select","verify"]),childOptions,async (_,signal)=>{
      checks++; if(cancel) controller.abort(); await delay(1000,undefined,{signal});
    },controller.signal,cancel?5000:100));
    if (cancel) assert.equal(checks,1);
    // A deadline can also expire during process startup; neither path may grant permission.
    else assert.ok(checks <= 1);
  }
});
test("a stalled host check cannot delay cancellation or grant a late permission", async () => {
  const controller = new AbortController(); let release;
  await assert.rejects(stagedCli(process.execPath, childArgs(["select", "verify"]), childOptions, async () => {
    controller.abort(); await new Promise(resolve => { release = resolve; });
  }, controller.signal, 1000));
  release();
});
