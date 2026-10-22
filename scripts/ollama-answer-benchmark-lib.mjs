import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import http from "node:http";
import { verifyCase } from "./local-answer-benchmark-lib.mjs";

export const PROFILE = "ollama-knowledge-answer-v14";
export const REUSE_PROFILE = "ollama-knowledge-answer-v15-reuse";
export const GUARDED_PROFILE = "ollama-knowledge-answer-v17-guarded";
export const QUESTION_PROFILE = "ollama-knowledge-answer-v16-question";
export const QUESTION_FIRST_PROFILE = "ollama-knowledge-answer-v18-question-first";
export const EVIDENCE_BOUND_PROFILE = "ollama-knowledge-answer-v19-evidence-bound";
export const EVIDENCE_ROLE_PROFILE = "ollama-knowledge-answer-v20-evidence-role";
export const RUNNER = "llamacpp";
export const REPORT_SCHEMA = "ollama-answer-quality-report-v2";
export const REUSE_REPORT_SCHEMA = "ollama-answer-quality-report-v3";
export const SUITES = { baseline: "knowledge-answer-synthetic-v1", challenge: "knowledge-answer-challenge-v1", coverage: "knowledge-answer-coverage-v1", extraction: "knowledge-answer-extraction-v1", decision: "knowledge-answer-decision-v1", mixed: "knowledge-answer-mixed-v1", availability: "knowledge-answer-availability-v1", support: "knowledge-answer-support-v1" };
const GiB = 1024 ** 3;
export const RESERVE_BYTES = 6 * GiB + 512 * 1024 ** 2;
export const RESOURCE_POLICY = "macos-pressure-reserve-6gib-buffer-512mib-load-1.5x-v1";
export const REUSE_RESOURCE_POLICY = "macos-pressure-reserve-6gib-buffer-512mib-load-1.5x-reuse-1gib-idle-30s-v1";
export const REUSE_HEADROOM_BYTES = GiB;
export function residencyPolicy(residency) {
  assert.ok(["cold", "reuse"].includes(residency));
  return residency === "cold"
    ? { profile: PROFILE, report_schema: REPORT_SCHEMA, resource_policy: RESOURCE_POLICY }
    : { profile: REUSE_PROFILE, report_schema: REUSE_REPORT_SCHEMA, resource_policy: REUSE_RESOURCE_POLICY };
}
export function executionPolicy(residency, candidate = "independent") {
  assert.ok(["independent", "question", "guarded", "question-first", "evidence-bound", "evidence-role"].includes(candidate));
  const policy = residencyPolicy(residency);
  if (candidate === "independent") return policy;
  assert.equal(residency, "reuse", "experimental candidate requires finite reuse");
  if (candidate === "evidence-role") return { ...policy, profile: EVIDENCE_ROLE_PROFILE, report_schema: "ollama-answer-quality-report-v8" };
  if (candidate === "evidence-bound") return { ...policy, profile: EVIDENCE_BOUND_PROFILE, report_schema: "ollama-answer-quality-report-v7" };
  if (candidate === "question-first") return { ...policy, profile: QUESTION_FIRST_PROFILE, report_schema: "ollama-answer-quality-report-v6" };
  return { ...policy, profile: candidate === "guarded" ? GUARDED_PROFILE : QUESTION_PROFILE, report_schema: candidate === "guarded" ? "ollama-answer-quality-report-v5" : "ollama-answer-quality-report-v4" };
}
export function localMetadata(endpoint, path, body, signal) {
  return new Promise((accept, reject) => {
    const bounded = signal ? AbortSignal.any([signal, AbortSignal.timeout(3000)]) : AbortSignal.timeout(3000);
    const request = http.request(new URL(path, endpoint), { method: body ? "POST" : "GET", signal: bounded, agent: false,
      headers: body ? { "content-type": "application/json" } : {} }, response => {
      if (response.statusCode !== 200) { response.destroy(); reject(new Error("local metadata unavailable")); return; }
      const chunks = []; let bytes = 0;
      response.on("data", chunk => {
        bytes += chunk.length;
        if (bytes > 1024 * 1024) { response.destroy(new Error("local metadata exceeds limit")); return; }
        chunks.push(chunk);
      });
      response.on("error", reject);
      response.on("end", () => { try { accept(JSON.parse(Buffer.concat(chunks).toString("utf8"))); } catch { reject(new Error("invalid local metadata")); } });
    });
    request.on("error", reject); request.end(body ? JSON.stringify(body) : undefined);
  });
}
function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") return Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])]));
  return value;
}
export function digest(value) { return createHash("sha256").update(JSON.stringify(canonical(value))).digest("hex"); }
export function localTarget(endpoint, model) {
  assert.match(endpoint, /^http:\/\/(127\.0\.0\.1|\[::1\]):[1-9][0-9]{0,4}$/);
  const url = new URL(endpoint);
  assert.ok(Number(url.port) > 0 && Number(url.port) <= 65535);
  assert.equal(url.origin, endpoint);
  assert.match(model, /^[a-zA-Z0-9._:/-]{1,128}$/);
  assert.ok(!/(^|[:/-])cloud($|[:/-])/i.test(model));
  return { endpoint, model_alias: model };
}
export function parseOptions(args) {
  const result = { endpoint: "http://127.0.0.1:11434", model: "qwen3.5:9b", suite: "baseline", residency: "reuse", candidate: "independent" };
  const seen = new Set();
  for (let i = 0; i < args.length; i += 2) {
    const key = args[i]?.replace(/^--/, "");
    assert.ok(["endpoint", "model", "suite", "residency", "candidate"].includes(key) && args[i] === `--${key}` && !seen.has(key) && typeof args[i + 1] === "string");
    seen.add(key); result[key] = args[i + 1];
  }
  assert.ok(Object.hasOwn(SUITES, result.suite));
  executionPolicy(result.residency, result.candidate);
  return { ...result, ...localTarget(result.endpoint, result.model) };
}
function supportedVersion(version) {
  assert.match(version, /^\d+\.\d+\.\d+(?:[a-zA-Z0-9.+-]{0,32})$/);
  const [, major, minor, patch] = /^(\d+)\.(\d+)\.(\d+)/.exec(version).map(Number);
  assert.ok(major > 0 || minor > 40 || minor === 40 && patch >= 1, "requires Ollama 0.40.1+ runner selection");
}
export function runtimeIdentity(target, version, tags, show) {
  localTarget(target.endpoint, target.model_alias);
  supportedVersion(version.version);
  const models = tags.models.filter(model => model.name === target.model_alias && model.model === target.model_alias && model.details?.runner === RUNNER);
  assert.equal(models.length, 1);
  const model = models[0];
  assert.match(model.digest, /^[a-f0-9]{64}$/);
  assert.ok(Number.isSafeInteger(model.size) && model.size > 0);
  assert.ok(!model.remote_model && !model.remote_host && !show.remote_model && !show.remote_host);
  assert.equal(model.details.format, "gguf");
  assert.equal(show.details.runner, RUNNER);
  const selected = show.manifests.filter(manifest => manifest.selected);
  assert.equal(selected.length, 1); assert.equal(selected[0].runner, RUNNER); assert.equal(selected[0].digest, `sha256:${model.digest}`);
  assert.ok(show.capabilities.includes("completion"));
  if (show.capabilities.includes("thinking")) assert.ok(show.thinking?.values?.includes(false));
  return { engine: "ollama", runner: RUNNER, version: version.version, execution_profile: executionPolicy(target.residency ?? "cold", target.candidate ?? "independent").profile, endpoint: target.endpoint, model_alias: target.model_alias,
    model_digest: model.digest, model_size: model.size, model_metadata_sha256: digest({ details: model.details, show }) };
}
export function availableMemory(total, pressure) {
  assert.ok(Number.isSafeInteger(total) && total > 0);
  const matches = [...pressure.matchAll(/^System-wide memory free percentage: ([0-9]{1,3})%\s*$/gm)];
  assert.equal(matches.length, 1);
  const percent = Number(matches[0][1]);
  assert.ok(percent <= 100);
  return Math.floor(total * percent / 100);
}
export function memoryBudget(available, modelSize, loading, headroom = 0) {
  assert.ok(Number.isSafeInteger(available) && available >= 0 && Number.isSafeInteger(modelSize) && modelSize > 0);
  assert.ok(headroom === 0 || headroom === REUSE_HEADROOM_BYTES);
  // Conservative estimate, not a GPU allocation guarantee on Apple unified memory.
  const required = RESERVE_BYTES + (loading ? Math.ceil(modelSize * 1.5) : headroom);
  if (available < required) throw Object.assign(new Error("local memory reserve unavailable"), { answerFailure: "resource" });
  return required;
}
// Read-only memory settling; never retries an inference or unloads a shared model.
export async function memoryReady(sample, modelSize, signal, settleMs = 15000, intervalMs = 500, loading = true) {
  const started = Date.now();
  for (;;) {
    if (signal?.aborted) throw new Error("cancelled before local inference");
    const available = await sample();
    try { memoryBudget(available, modelSize, loading, loading ? 0 : REUSE_HEADROOM_BYTES); return { available, waited_ms: Date.now() - started }; }
    catch (error) {
      if (error.answerFailure !== "resource" || Date.now() - started >= settleMs) throw error;
    }
    await delay(intervalMs, undefined, { signal });
  }
}
export async function guardedSend(send, check, signal, intervalMs = 500) {
  const controller = new AbortController();
  const halt = () => controller.abort();
  signal?.addEventListener("abort", halt, { once: true });
  if (signal?.aborted) halt();
  let pending = null, failure;
  const sample = async () => {
    try { await check(); }
    catch { failure = Object.assign(new Error("local memory reserve unconfirmed"), { answerFailure: "resource" }); halt(); }
  };
  const timer = setInterval(() => { if (!pending) pending = sample().finally(() => { pending = null; }); }, intervalMs);
  try {
    const result = await send(controller.signal);
    clearInterval(timer);
    if (pending) await pending;
    await sample();
    if (failure) throw failure;
    return result;
  } catch (error) { throw failure ?? error; }
  finally {
    clearInterval(timer); signal?.removeEventListener("abort", halt);
    if (pending) await pending;
  }
}

// A local audit gate, not a signed attestation or private execution authorization.
export function qualityGate(reports, expected) {
  assert.equal(reports.length, Object.keys(SUITES).length * 2, "requires two reports of each of the eight frozen suites");
  assert.deepEqual(Object.keys(expected).sort(), Object.keys(SUITES).sort());
  const rounds = Object.fromEntries(Object.keys(SUITES).map(suite => [suite, []])), ids = new Set();
  const runtime = reports[0].runtime;
  const guarded = runtime.execution_profile === GUARDED_PROFILE;
  const questionFirst = runtime.execution_profile === QUESTION_FIRST_PROFILE;
  const evidenceBound = runtime.execution_profile === EVIDENCE_BOUND_PROFILE;
  const evidenceRole = runtime.execution_profile === EVIDENCE_ROLE_PROFILE;
  const nativeRequests = guarded || questionFirst || evidenceBound || evidenceRole ? 3 : 2;
  const question = runtime.execution_profile === QUESTION_PROFILE;
  const reuse = guarded || question || questionFirst || evidenceBound || evidenceRole || runtime.execution_profile === REUSE_PROFILE;
  const policy = executionPolicy(reuse ? "reuse" : "cold", evidenceRole ? "evidence-role" : evidenceBound ? "evidence-bound" : questionFirst ? "question-first" : guarded ? "guarded" : question ? "question" : "independent");
  assert.deepEqual(Object.keys(runtime).sort(), ["arch", "endpoint", "engine", "execution_profile", "model_alias", "model_digest", "model_metadata_sha256", "model_size", "platform", "resource_policy", "runner", "total_memory", "version"]);
  assert.equal(runtime.engine, "ollama"); assert.equal(runtime.execution_profile, policy.profile);
  assert.equal(runtime.runner, RUNNER);
  assert.equal(runtime.platform, "darwin"); assert.equal(runtime.arch, "arm64"); assert.equal(runtime.resource_policy, policy.resource_policy);
  supportedVersion(runtime.version);
  for (const field of ["model_digest", "model_metadata_sha256"]) assert.match(runtime[field], /^[a-f0-9]{64}$/);
  assert.ok(Number.isSafeInteger(runtime.total_memory) && runtime.total_memory > RESERVE_BYTES);
  localTarget(runtime.endpoint, runtime.model_alias);
  const intervals = [];
  for (const report of reports) {
    assert.deepEqual(Object.keys(report).sort(), ["complete", "ended_at", "exit_code", "manifests", "not_run", "passed_cases", "resources", "results", "run_id", "runtime", "schema", "started_at", "suite", "synthetic_only", "total_cases"]);
    assert.equal(report.schema, policy.report_schema); assert.equal(report.synthetic_only, true);
    assert.match(report.run_id, /^[a-f0-9-]{36}$/); assert.ok(!ids.has(report.run_id)); ids.add(report.run_id);
    assert.deepEqual(report.runtime, runtime);
    assert.ok(Object.hasOwn(rounds, report.suite));
    assert.deepEqual(report.manifests, expected[report.suite]);
    assert.equal(report.total_cases, report.manifests.length);
    assert.equal(report.passed_cases, report.results.filter(result => result.quality_pass).length);
    const start = Date.parse(report.started_at), end = Date.parse(report.ended_at);
    assert.ok(Number.isFinite(start) && Number.isFinite(end) && end >= start);
    intervals.push([start, end]);
    assert.equal(report.complete, true); assert.deepEqual(report.not_run, []);
    assert.equal(report.results.length, report.manifests.length);
    assert.equal(report.exit_code, report.passed_cases === report.total_cases ? 0 : 2);
    const resourceKeys = ["inflight_checks", "minimum_inflight_available_bytes", "minimum_preload_available_bytes", "preload_checks", "settle_wait_ms"];
    if (reuse) resourceKeys.push("cold_checks", "reload_checks", "reuse_checks", "minimum_loading_available_bytes", "minimum_reuse_available_bytes");
    assert.deepEqual(Object.keys(report.resources).sort(), resourceKeys.sort());
    for (const [key, value] of Object.entries(report.resources)) {
      if (reuse && ["minimum_loading_available_bytes", "minimum_reuse_available_bytes"].includes(key) && value === null) continue;
      assert.ok(Number.isSafeInteger(value) && value >= 0);
    }
    assert.equal(report.resources.preload_checks, report.total_cases * nativeRequests);
    assert.ok(report.resources.inflight_checks >= report.total_cases);
    if (reuse) {
      const resources = report.resources, loadingChecks = resources.cold_checks + resources.reload_checks;
      assert.equal(loadingChecks + resources.reuse_checks, resources.preload_checks);
      assert.ok(resources.reuse_checks > 0, "reuse reports must observe actual resident requests");
      assert.equal(resources.minimum_loading_available_bytes === null, loadingChecks === 0);
      if (loadingChecks > 0) memoryBudget(resources.minimum_loading_available_bytes, runtime.model_size, true);
      memoryBudget(resources.minimum_reuse_available_bytes, runtime.model_size, false, REUSE_HEADROOM_BYTES);
      assert.equal(resources.minimum_preload_available_bytes, Math.min(...[resources.minimum_loading_available_bytes, resources.minimum_reuse_available_bytes].filter(value => value !== null)));
    } else memoryBudget(report.resources.minimum_preload_available_bytes, runtime.model_size, true);
    memoryBudget(report.resources.minimum_inflight_available_bytes, runtime.model_size, false);
    assert.ok(!Object.hasOwn(report, "failure") && !Object.hasOwn(report, "failed_case"));
    report.results.forEach((result, i) => {
      const manifest = report.manifests[i];
      const raw = { manifest, endpoint: runtime.endpoint, model: runtime.model_alias, synthetic_only: true,
        elapsed_ms: result.elapsed_ms, protocol_valid: result.protocol_valid,
        evaluation: { citation_valid: result.citation_valid, quality_pass: result.quality_pass, checks: result.checks } };
      assert.deepEqual(result, verifyCase(raw, manifest, runtime));
    });
    rounds[report.suite].push(report);
  }
  intervals.sort((a, b) => a[0] - b[0]);
  for (let i = 1; i < intervals.length; i++) assert.ok(intervals[i][0] > intervals[i - 1][1], "rounds must be sequential");
  for (const [suite, list] of Object.entries(rounds)) {
    assert.equal(list.length, 2);
    assert.ok(expected[suite].length > 0);
    for (const manifest of expected[suite]) {
      assert.equal(manifest.execution_profile, policy.profile); assert.equal(manifest.case.suite, SUITES[suite]);
      assert.equal(manifest.case.synthetic_only, true);
    }
  }
  const failed = reports.flatMap(report => report.results.filter(result => !result.quality_pass).map(result => ({ suite: report.suite, run_id: report.run_id, id: result.id })));
  return { schema: evidenceRole ? "ollama-answer-quality-gate-v14" : evidenceBound ? "ollama-answer-quality-gate-v13" : questionFirst ? "ollama-answer-quality-gate-v12" : guarded ? "ollama-answer-quality-gate-v11" : question ? "ollama-answer-quality-gate-v10" : reuse ? "ollama-answer-quality-gate-v9" : "ollama-answer-quality-gate-v8", synthetic_only: true, runtime,
    reports_sha256: reports.map(digest), manifests_sha256: digest(expected),
    rounds_per_suite: 2, native_requests_per_case: nativeRequests, evaluated_cases: reports.reduce((sum, report) => sum + report.total_cases, 0),
    passed: failed.length === 0, failed_cases: failed, private_execution_authorized: false };
}
