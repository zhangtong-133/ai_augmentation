import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, stat, symlink } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { verifyCase, runCases } from "./local-value-benchmark-lib.mjs";
import { newReportDirectory, writeReport } from "./local-report.mjs";
const target = { endpoint: "http://127.0.0.1:11435", model_alias: "fixed" };
const manifest = { id: "a", corpus_sha256: "a".repeat(64), input_digest: "b".repeat(64), request_sha256: "c".repeat(64),
  execution_profile: "local-rss-v1", quality_version: "rss-quality-v1", prompt_bytes: 100, items: ["rust", "weather"], checks: [{ kind: "minimum", item: "rust", value: 60 }] };
function output(m = manifest, passed = true) {
  return { endpoint: target.endpoint, model: target.model_alias, elapsed_ms: 12, response_bytes: 80, synthetic_only: true,
    result: { manifest: structuredClone(m), protocol_valid: true, quality_pass: passed, checks: [{ index: 0, passed }], scores: { rust: 90, weather: null } } };
}
test("reports reject changed request/corpus/profile/target and unchecked result fields", () => {
  assert.equal(verifyCase(output(), manifest, target).quality_pass, true);
  const mutations = [r => r.result.manifest.request_sha256 = "d".repeat(64), r => r.result.manifest.corpus_sha256 = "d".repeat(64),
    r => r.result.manifest.execution_profile = "other", r => r.model = "other", r => r.endpoint = "http://127.0.0.1:1234",
    r => delete r.result.scores.weather, r => r.result.checks.push(r.result.checks[0]), r => r.result.checks[0].index = 1,
    r => r.result.quality_pass = false, r => r.result.scores.rust = 101, r => r.result.reason = "private model text"];
  for (const mutate of mutations) { const r = output(); mutate(r); assert.throws(() => verifyCase(r, manifest, target)); }
});
test("quality failure is recorded without retry; protocol uncertainty stops later cases", async () => {
  const manifests = [manifest, { ...manifest, id: "b" }, { ...manifest, id: "c" }];
  let sent = [];
  const quality = await runCases(manifests, target, async id => { sent.push(id); return output(manifests.find(m => m.id === id), id !== "b"); });
  assert.deepEqual(sent, ["a", "b", "c"]); assert.equal(quality.complete, true); assert.equal(quality.exit_code, 2);
  sent = [];
  const unknown = await runCases(manifests, target, async id => { sent.push(id); if (id === "b") throw new Error("raw transport text must not escape"); return output(manifest); });
  assert.deepEqual(sent, ["a", "b"]); assert.equal(unknown.exit_code, 1); assert.equal(unknown.complete, false);
  assert.equal(unknown.failed_case, "b"); assert.equal(unknown.results.length, 1);
  assert.ok(!JSON.stringify(unknown).includes("raw transport"));
  const diagnosed = await runCases([manifest], target, async () => { throw Object.assign(new Error("secret"), { benchmarkFailure: "output_ids" }); });
  assert.equal(diagnosed.failure, "output_ids"); assert.equal(diagnosed.exit_code, 1);
});
test("each report is private, fresh, bounded and cannot replace a previous result or follow a base symlink", async () => {
  const temporary = await mkdtemp(join(tmpdir(), "local-report-test-"));
  try {
    const base = join(temporary, "private");
    const first = await newReportDirectory(base, "quality"), second = await newReportDirectory(base, "quality");
    assert.notEqual(first, second); assert.equal((await stat(first)).mode & 0o777, 0o700);
    const file = await writeReport(first, { passed: false });
    assert.equal((await stat(file)).mode & 0o777, 0o600);
    await assert.rejects(writeReport(first, { passed: true }), { code: "EEXIST" });
    assert.equal(JSON.parse(await readFile(file, "utf8")).passed, false);
    await assert.rejects(writeReport(second, { text: "x".repeat(65536) }));
    await symlink(base, join(temporary, "link"));
    await assert.rejects(newReportDirectory(join(temporary, "link"), "quality"));
  } finally { await rm(temporary, { recursive: true, force: true }); }
});

test("suite/profile options are explicit, bounded and reject duplicates", async () => {
  const { benchmarkOptions } = await import("./local-benchmark-options.mjs");
  assert.equal(benchmarkOptions(["run"]).suite, "baseline");
  assert.equal(benchmarkOptions(["run", "--model", "qwen3-8b", "--suite", "challenge"]).model, "qwen3-8b");
  assert.equal(benchmarkOptions(["run", "--suite", "challenge", "--profile", "local-rss-v1"]).profile, "local-rss-v1");
  assert.equal(benchmarkOptions(["run", "--profile", "local-rss-v2", "--suite", "challenge"]).suite, "challenge");
  for (const flags of [["--model", "unknown"], ["--model", "qwen3-8b", "--model", "qwen3-4b"], ["--suite", "private.json"], ["--profile", "unknown"], ["--suite", "baseline", "--suite", "challenge"]]) {
    assert.throws(() => benchmarkOptions(["run", ...flags]));
  }
});
