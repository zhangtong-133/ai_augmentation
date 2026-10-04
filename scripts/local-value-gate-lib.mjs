import assert from "node:assert/strict";
import { compareReports, validateReport } from "./local-value-compare-lib.mjs";

// expected is generated offline by the current Rust evaluator, never by an input report.
export function qualityGate(inputs, expected) {
  assert.ok(Array.isArray(inputs) && inputs.length >= 1 && inputs.length <= 10);
  assert.deepEqual(Object.keys(expected).sort(), ["baseline", "challenge"]);
  const reports = inputs.map(validateReport), runtime = reports[0].runtime;
  const groups = { baseline: [], challenge: [] };
  for (const report of reports) {
    assert.equal(report.run_scope, "full_corpus");
    assert.deepEqual(report.runtime, runtime, "different runtime or model");
    const suite = report.manifests[0].quality_version === "rss-quality-v1" ? "baseline" : "challenge";
    assert.deepEqual(report.manifests, expected[suite], "not the current frozen suite/profile/request");
    assert.equal(report.manifests[0].execution_profile, "local-rss-v2");
    groups[suite].push(report);
  }
  // A copied report or overlapping run cannot count as another sequential benchmark.
  const ordered = [...reports].sort((a, b) => Date.parse(a.started_at) - Date.parse(b.started_at));
  for (let i = 1; i < ordered.length; i++) {
    assert.ok(Date.parse(ordered[i].started_at) > Date.parse(ordered[i - 1].started_at));
    assert.ok(Date.parse(ordered[i].started_at) >= Date.parse(ordered[i - 1].ended_at));
  }
  const suites = Object.entries(groups).map(([suite, runs]) => ({
    suite, required_runs: 2, supplied_runs: runs.length,
    complete_runs: runs.filter(r => r.complete).length,
    passed_runs: runs.filter(r => r.exit_code === 0).length,
    passed: runs.length >= 2 && runs.every(r => r.exit_code === 0),
    comparison: runs.length >= 2 ? compareReports(runs) : null,
  }));
  return { schema: "rss-synthetic-quality-gate-v1", synthetic_only: true,
    execution_profile: "local-rss-v2", runtime,
    synthetic_gate_passed: suites.every(s => s.passed),
    suites, configuration_changed: false, real_material_quality_verified: false,
    gaming_reserve_verified: false };
}
