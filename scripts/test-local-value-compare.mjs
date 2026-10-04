import { test } from "node:test";
import assert from "node:assert/strict";
import { compareReports, validateReport } from "./local-value-compare-lib.mjs";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, writeFile, rm, symlink } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
const execute = promisify(execFile);
import { report } from "./quality-test-fixture.mjs";
test("same conditions report exact repetition separately from quality and timing", () => {
  const a = report(), b = report("2026-10-04T12:10:00.000Z"); b.results[0].elapsed_ms = 1200;
  const same = compareReports([a, b]);
  assert.equal(same.all_passed, true); assert.equal(same.repeated_identically, true);
  assert.deepEqual(same.cases[0].elapsed_ms, { min: 400, max: 1200 });
  b.results[0].scores.article = 81;
  const changed = compareReports([a, b]); assert.equal(changed.all_passed, true); assert.equal(changed.repeated_identically, false);
});
test("abstention, protocol failure and not-run stay distinct and cannot improve the pass count", () => {
  const a = report(), b = report("2026-10-04T12:10:00.000Z");
  a.results[0].scores.article = null; a.results[0].quality_pass = false; a.results[0].checks[0].passed = false; a.passed_cases = 2; a.exit_code = 2;
  b.complete = false; b.results = b.results.slice(0, 1); b.passed_cases = 1; b.exit_code = 1; b.failed_case = "b"; b.failure = "output_reason_category";
  const compared = compareReports([a, b]); assert.equal(compared.all_passed, false);
  assert.deepEqual(compared.cases[0].scores.article, { numeric_runs: 1, abstained_runs: 1, min: 80, max: 80 });
  assert.equal(compared.cases[1].protocol_failed_runs, 1); assert.equal(compared.cases[1].completed_runs, 1);
  assert.equal(compared.cases[2].not_run, 1); assert.equal(compared.cases[2].scores.article.numeric_runs, 1);
  assert.equal(compared.cases[2].repeated_identically, false);
});
test("different request, model, profile or criteria cannot be pooled as repeated runs", () => {
  for (const mutate of [r => r.manifests[0].request_sha256 = "f".repeat(64), r => r.runtime.model_sha256 = "e".repeat(64),
    r => r.manifests.forEach(m => m.execution_profile = "local-rss-v1"), r => r.manifests[0].checks[0].value = 59,
    r => r.manifests.forEach(m => m.corpus_sha256 = "2".repeat(64)), r => r.runtime.server_sha256 = "3".repeat(64)]) {
    const changed = report(); mutate(changed); assert.throws(() => compareReports([report(), changed]));
  }
});
test("inconsistent counters, duplicate or reordered cases and arbitrary output fields are rejected", () => {
  for (const mutate of [r => r.passed_cases = 2, r => r.results[0].quality_pass = false, r => r.results.reverse(),
    r => r.manifests[1].id = "a", r => r.results[0].scores.article = 101, r => r.results[0].reason = "private text",
    r => r.run_scope = "independent_case", r => r.total_cases = 4, r => r.failure = "untrusted text", r => r.results[0].scores.article = 0]) {
    const invalid = report(); mutate(invalid); assert.throws(() => validateReport(invalid));
  }
});
test("CLI refuses duplicate, symlinked, invalid UTF-8 and oversized reports without echoing their content", async () => {
  const directory = await mkdtemp(join(tmpdir(), "quality-compare-test-"));
  const cli = fileURLToPath(new URL("./local-value-compare.mjs", import.meta.url));
  try {
    const valid = join(directory, "valid.json"); await writeFile(valid, JSON.stringify(report()));
    const bad = join(directory, "bad.json"), link = join(directory, "link.json");
    await symlink(valid, link);
    for (const value of [Buffer.from("private output must not escape"), Buffer.from([0xff, 0xfe]), Buffer.alloc(65537, 32)]) {
      await writeFile(bad, value);
      await assert.rejects(execute(process.execPath, [cli, valid, bad], { timeout: 5000 }), e => e.code === 1 && e.stdout === "" && !e.stderr.includes("private output"));
    }
    for (const path of [valid, link]) await assert.rejects(execute(process.execPath, [cli, valid, path], { timeout: 5000 }), e => e.code === 1 && e.stdout === "");
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test("challenge reports compare separately and cannot mix suites within or across runs", () => {
  const a = report(), b = report("2026-10-04T12:10:00.000Z");
  a.manifests.forEach(m => m.quality_version = "rss-challenge-v1");
  b.manifests.forEach(m => m.quality_version = "rss-challenge-v1");
  assert.equal(compareReports([a, b]).all_passed, true);
  assert.throws(() => compareReports([a, report()]));
  b.manifests[0].quality_version = "rss-quality-v1";
  assert.throws(() => validateReport(b));
});
