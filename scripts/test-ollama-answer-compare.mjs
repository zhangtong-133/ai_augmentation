import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { promisify } from "node:util";
import { execFile } from "node:child_process";
import { fileURLToPath } from "node:url";
import { answerReports, failCase } from "./answer-quality-test-fixture.mjs";
import { compareQualityReports } from "./ollama-answer-compare-lib.mjs";
import { comparisonFiles, expectedManifests, readQualityReports } from "./ollama-answer-reports.mjs";
import { writeReport } from "./local-report.mjs";
import { SUITES, executionPolicy } from "./ollama-answer-benchmark-lib.mjs";

const execute = promisify(execFile);
const pair = () => [answerReports(), answerReports({ candidate: "guarded", offset: 100 })];
const compare = (a, b) => compareQualityReports(a.reports, b.reports, a.expected, b.expected);
const aProfile = executionPolicy("reuse").profile;

test("all 71 cases retain check regressions even when whole-case quality improves", () => {
  const [a, b] = pair();
  failCase(a.reports, "baseline", "injection", ["expected_citations", "forbidden_terms"]);
  failCase(b.reports, "baseline", "injection", ["expected_status", "required_terms", "expected_citations"]);
  for (const [suite, id] of [["extraction", "same_source_missing"], ["support", "price_availability_undetermined"], ["support", "two_absences_establish_availability"]]) {
    failCase(a.reports, suite, id, ["expected_status"]);
  }
  const result = compare(a, b);
  assert.equal(result.boolean_checks, 568); assert.equal(result.cases.length, 71);
  assert.deepEqual(result.quality, { passed: 134, failed: 2, improved: 6, regressed: 0 });
  assert.equal(result.checks.expected_status.regressed + result.checks.required_terms.regressed, 4);
  assert.deepEqual(result.cases.find(result => result.id === "injection"), {
    suite: "baseline", id: "injection", quality: ["failed", "failed"], citation_valid: ["passed", "passed"],
    checks: { expected_status: ["regressed", "regressed"], required_terms: ["regressed", "regressed"],
      expected_citations: ["failed", "failed"], forbidden_terms: ["improved", "improved"] },
  });
  assert.equal(result.all_passed, false); assert.equal(result.right.quality_gate_passed, false);
  assert.equal(result.private_execution_authorized, false);
  assert.equal(result.left.native_requests_per_case, 2); assert.equal(result.right.native_requests_per_case, 3);
  assert.throws(() => compareQualityReports([result, ...a.reports.slice(1)], b.reports, a.expected, b.expected));
});

test("rounds pair chronologically within each suite regardless of argument order", () => {
  const [a, b] = pair();
  b.reports[0].results[0].elapsed_ms = 90000;
  // One observation differs so an accidental pairing change would be visible.
  b.reports[0].results[0].checks[0].passed = false; b.reports[0].results[0].quality_pass = false;
  b.reports[0].passed_cases--; b.reports[0].exit_code = 2;
  const expected = compare(a, b);
  a.reports.reverse(); b.reports.reverse();
  assert.deepEqual(compare(a, b), expected);
  assert.deepEqual(expected.cases[0].quality, ["regressed", "passed"]);
  assert.equal(expected.quality.regressed, 1);
  assert.equal(expected.left.median_elapsed_ms, 100); assert.equal(expected.right.median_elapsed_ms, 100);
});

test("cold, reuse, question and guarded histories keep separate gates and cost metadata", () => {
  for (const options of [{ residency: "cold" }, {}, { candidate: "question" }, { candidate: "guarded" }]) {
    const a = answerReports(options), b = answerReports({ ...options, offset: 100 });
    const result = compare(a, b);
    assert.equal(result.left.execution_profile, executionPolicy(options.residency ?? "reuse", options.candidate).profile);
    assert.equal(result.all_passed, true); assert.equal(result.private_execution_authorized, false);
  }
  assert.equal(compare(answerReports({ residency: "cold" }), answerReports({ offset: 100 })).all_passed, true);
});

test("different runtime identities, conditions, requests and mixed profiles are not comparable", () => {
  for (const field of ["version", "endpoint", "model_alias", "model_digest", "model_metadata_sha256", "model_size", "total_memory"]) {
    const [a, b] = pair();
    for (const report of b.reports) {
      const value = report.runtime[field];
      report.runtime[field] = typeof value === "number" ? value + 1000 : field === "version" ? "0.40.4" : field === "endpoint" ? "http://127.0.0.1:11435" : field.endsWith("sha256") || field.endsWith("digest") ? "c".repeat(64) : "other-model";
    }
    assert.throws(() => compare(a, b));
  }
  for (const mutate of [
    b => b.expected.baseline[0].case.required_terms.push("changed"),
    b => b.expected.baseline[0].case.corpus_sha256 = "d".repeat(64),
    b => b.expected.baseline[0].case.citation_ids = [2],
  ]) {
    const [a, b] = pair(); mutate(b);
    b.reports.filter(report => report.suite === "baseline").forEach(report => { report.manifests = structuredClone(b.expected.baseline); });
    assert.throws(() => compare(a, b));
  }
  for (const mutate of [
    b => b.reports[0].manifests[0].request_sha256 = "d".repeat(64),
    b => b.reports[0].runtime.execution_profile = aProfile,
    b => b.reports[0].resources.preload_checks--,
    b => b.reports[0].diagnostic_only = true,
    b => b.reports[0].results[0].semantic_diagnostic = {},
    b => b.reports[0].answer = "private model text",
    b => b.reports[0].complete = false,
    b => b.reports.pop(),
    b => b.reports[1] = structuredClone(b.reports[0]),
  ]) {
    const [a, b] = pair(); mutate(b); assert.throws(() => compare(a, b));
  }
});

test("reused run ids and overlapping candidate observations cannot masquerade as independent rounds", () => {
  const a = answerReports();
  assert.throws(() => compare(a, structuredClone(a)));
  const b = answerReports({ candidate: "guarded" });
  b.reports.forEach((report, index) => { report.run_id = `11111111-0000-4000-8000-${String(index).padStart(12, "0")}`; });
  assert.throws(() => compare(a, b));
});

test("full worst-case transitions fit private report bounds and contain no corpus or model text", async () => {
  const [a, b] = pair();
  for (const report of b.reports) for (const result of report.results) {
    failCase(b.reports, report.suite, result.id, result.checks.map(check => check.name));
  }
  const value = compare(a, b), directory = await mkdtemp(join(tmpdir(), "answer-comparison-output-"));
  try {
    const path = await writeReport(directory, value), bytes = await readFile(path);
    assert.ok(bytes.length <= 65536); assert.equal(JSON.parse(bytes).quality.regressed, 142);
    for (const forbidden of ["蓝鲸", "CANARY_BAD_7", '"corpus_sha256"', '"exact_excerpts"', '"evidence"', '"answer"', '"quote"']) assert.ok(!bytes.toString().includes(forbidden));
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test("expected manifests are rebuilt for each exact profile without runtime or model access", async () => {
  for (const options of [{ residency: "cold" }, {}, { candidate: "question" }, { candidate: "guarded" }]) {
    const { reports, expected } = answerReports(options), calls = [];
    assert.deepEqual(await expectedManifests(reports, async (target, suite) => {
      calls.push(suite); assert.equal(target.residency, options.residency ?? "reuse");
      assert.equal(target.candidate, options.candidate ?? "independent"); return expected[suite];
    }), expected);
    assert.deepEqual(calls, Object.keys(SUITES));
  }
  const { reports } = answerReports(); reports[0].runtime.execution_profile = "unrecognized";
  await assert.rejects(expectedManifests(reports, () => assert.fail("must reject before loading")));
});

test("report readers reject outside paths, duplicate bytes, symlinks, invalid UTF-8 and oversized files", async () => {
  const directory = await mkdtemp(join(tmpdir(), "answer-comparison-input-")), base = join(directory, "local"), quality = join(base, "quality");
  try {
    await mkdir(quality, { recursive: true });
    const reports = pair().flatMap(value => value.reports), files = [];
    for (const [index, report] of reports.entries()) {
      const path = join(quality, `${index}.json`); await writeFile(path, JSON.stringify(report)); files.push(path);
    }
    assert.equal((await readQualityReports(files, base)).length, 32);
    assert.equal((await readQualityReports(files.slice(0, 16), base)).length, 16);
    const alias = join(directory, "alias"), linkedQuality = join(directory, "linked-quality");
    await symlink(base, alias); await mkdir(linkedQuality); await symlink(quality, join(linkedQuality, "quality"));
    for (const redirected of [alias, linkedQuality]) await assert.rejects(readQualityReports(files.slice(0, 16), redirected));
    const bad = join(quality, "bad.json"), link = join(quality, "link.json"), outside = join(directory, "outside.json");
    await symlink(files[0], link); await writeFile(outside, JSON.stringify(reports[0]));
    for (const path of [outside, link, files[1]]) await assert.rejects(readQualityReports([path, ...files.slice(1, 16)], base));
    for (const data of [await readFile(files[1]), Buffer.from([0xff, 0xfe]), Buffer.alloc(65537, 32), Buffer.from("private output must not escape")]) {
      await writeFile(bad, data); await assert.rejects(readQualityReports([bad, ...files.slice(1, 16)], base));
    }
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test("comparison syntax requires exactly two groups of sixteen and CLI errors never echo input", async () => {
  const left = Array.from({ length: 16 }, (_, i) => `/left/${i}.json`), right = Array.from({ length: 16 }, (_, i) => `/right/${i}.json`);
  const args = ["--left", ...left, "--right", ...right];
  assert.deepEqual(comparisonFiles(args), { left, right });
  for (const invalid of [[], args.slice(1), [...args, "extra"], ["--right", ...left, "--left", ...right], ["--left", ...left.slice(1), "--right", ...right], ["--left", ...left, "--right", ...right.slice(1), "--unknown"]]) assert.throws(() => comparisonFiles(invalid));
  const cli = fileURLToPath(new URL("./ollama-answer-benchmark.mjs", import.meta.url));
  for (const invalid of [[], ["--left", "private output must not escape"], args]) {
    await assert.rejects(execute(process.execPath, [cli, "compare", ...invalid], { timeout: 5000 }), error => error.code === 1 && error.stdout === "" && !error.stderr.includes("private output"));
  }
});
