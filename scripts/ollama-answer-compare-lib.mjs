import assert from "node:assert/strict";
import { digest, qualityGate, SUITES } from "./ollama-answer-benchmark-lib.mjs";

const names = ["expected_status", "required_terms", "expected_citations", "forbidden_terms"];
const counts = () => ({ passed: 0, failed: 0, improved: 0, regressed: 0 });
const transition = (before, after) => before ? after ? "passed" : "regressed" : after ? "improved" : "failed";
const identity = runtime => Object.fromEntries(Object.entries(runtime).filter(([key]) => !["execution_profile", "resource_policy"].includes(key)));
const ordered = reports => Object.keys(SUITES).flatMap(suite => reports.filter(report => report.suite === suite)
  .sort((a, b) => Date.parse(a.started_at) - Date.parse(b.started_at)));

function summary(reports, gate) {
  const elapsed = reports.flatMap(report => report.results.map(result => result.elapsed_ms)).sort((a, b) => a - b);
  const middle = Math.floor(elapsed.length / 2);
  return { execution_profile: gate.runtime.execution_profile, resource_policy: gate.runtime.resource_policy,
    gate_schema: gate.schema, quality_gate_passed: gate.passed, manifests_sha256: gate.manifests_sha256,
    native_requests_per_case: gate.native_requests_per_case, evaluated_cases: gate.evaluated_cases,
    passed_cases: reports.reduce((total, report) => total + report.passed_cases, 0),
    median_elapsed_ms: elapsed.length % 2 ? elapsed[middle] : (elapsed[middle - 1] + elapsed[middle]) / 2,
    reports: reports.map((report, index) => ({ suite: report.suite, round: index % 2 + 1,
      run_id: report.run_id, sha256: digest(report) })) };
}

// Compare complete observations without pooling profiles or treating agreement as truth.
// Current CLI manifests, supplied separately for each profile, remain the gate authority.
export function compareQualityReports(leftInputs, rightInputs, leftExpected, rightExpected) {
  const leftGate = qualityGate(leftInputs, leftExpected), rightGate = qualityGate(rightInputs, rightExpected);
  const left = ordered(leftInputs), right = ordered(rightInputs);
  assert.deepEqual(identity(leftGate.runtime), identity(rightGate.runtime), "runtime or model changed");
  const all = [...left, ...right];
  assert.equal(new Set(all.map(report => report.run_id)).size, all.length, "reused run is not another observation");
  const intervals = all.map(report => [Date.parse(report.started_at), Date.parse(report.ended_at)]).sort((a, b) => a[0] - b[0]);
  for (let index = 1; index < intervals.length; index++) assert.ok(intervals[index][0] > intervals[index - 1][1], "candidate observations overlap");
  for (const suite of Object.keys(SUITES)) {
    assert.deepEqual(leftExpected[suite].map(manifest => manifest.case), rightExpected[suite].map(manifest => manifest.case), "corpus or evaluation conditions changed");
  }
  const quality = counts(), citation = counts(), checks = Object.fromEntries(names.map(name => [name, counts()]));
  const suites = [], cases = [];
  for (const suite of Object.keys(SUITES)) {
    const before = left.filter(report => report.suite === suite), after = right.filter(report => report.suite === suite);
    const suiteQuality = counts();
    suites.push({ suite, cases_per_round: before[0].total_cases,
      left_passed: before.map(report => report.passed_cases), right_passed: after.map(report => report.passed_cases), quality: suiteQuality });
    for (let index = 0; index < before[0].results.length; index++) {
      const result = { suite, id: before[0].results[index].id, quality: [], citation_valid: [], checks: Object.fromEntries(names.map(name => [name, []])) };
      for (let round = 0; round < 2; round++) {
        const a = before[round].results[index], b = after[round].results[index];
        assert.equal(a.id, b.id);
        const changed = transition(a.quality_pass, b.quality_pass);
        result.quality.push(changed); quality[changed]++; suiteQuality[changed]++;
        const validity = transition(a.citation_valid, b.citation_valid);
        result.citation_valid.push(validity); citation[validity]++;
        names.forEach((name, check) => {
          const changed = transition(a.checks[check].passed, b.checks[check].passed);
          result.checks[name].push(changed); checks[name][changed]++;
        });
      }
      cases.push(result);
    }
  }
  // Canonical ordering makes command argument order irrelevant to pairing and digests.
  return { schema: "ollama-answer-quality-comparison-v1", synthetic_only: true, comparable: true,
    runtime: identity(leftGate.runtime), pairing: "suite_then_started_at", rounds_per_suite: 2,
    left: summary(left, leftGate), right: summary(right, rightGate),
    all_passed: leftGate.passed && rightGate.passed, private_execution_authorized: false,
    boolean_checks: leftGate.evaluated_cases * names.length, quality, citation_valid: citation, checks, suites, cases };
}
