import assert from "node:assert/strict";
import { compareReports, validateReport } from "./local-value-compare-lib.mjs";
const suites = { "rss-public-calibration-v1": "public_calibration", "rss-public-holdout-v1": "public_holdout" };
function diagnostics(runs, manifests) {
  return manifests.map((manifest, index) => {
    const counts = { expected_items: runs.length * manifest.items.length, observed_items: 0,
      numeric_items: 0, abstained_items: 0, failed_case_items: 0, not_run_items: 0,
      required_observed: 0, required_scored: 0, unexpected_abstentions: 0,
      below_minimum: 0, above_ceiling: 0, partial_observed: 0, partial_correct: 0,
      partial_abstained: 0, partial_overrated: 0, partial_underrated: 0 };
    for (const run of runs) {
      const result = run.results[index];
      for (const item of manifest.items) {
        if (!result) { counts[run.failed_case === manifest.id ? "failed_case_items" : "not_run_items"]++; continue; }
        counts.observed_items++;
        const score = result.scores[item];
        counts[score === null ? "abstained_items" : "numeric_items"]++;
        const minimums = manifest.checks.filter(c => c.kind === "minimum" && c.item === item).map(c => c.value);
        const ceilings = manifest.checks.filter(c => c.kind === "ceiling" && c.item === item).map(c => c.value);
        const minimum = minimums.length ? Math.max(...minimums) : null;
        const ceiling = ceilings.length ? Math.min(...ceilings) : null;
        if (minimum !== null) {
          counts.required_observed++;
          counts[score === null ? "unexpected_abstentions" : "required_scored"]++;
          if (score !== null && score < minimum) counts.below_minimum++;
        }
        if (score !== null && ceiling !== null && score > ceiling) counts.above_ceiling++;
        if (minimum === 40 && ceiling === 40) {
          counts.partial_observed++;
          counts[score === null ? "partial_abstained" : score === 40 ? "partial_correct" : score > 40 ? "partial_overrated" : "partial_underrated"]++;
        }
      }
    }
    return { id: manifest.id, ...counts,
      required_score_rate: counts.required_observed ? counts.required_scored / counts.required_observed : null,
      completion_rate: counts.expected_items ? counts.observed_items / counts.expected_items : null };
  });
}
// expected must come from the current offline Rust evaluator, never from the reports.
export function publicGate(inputs, expected) {
  assert.ok(Array.isArray(inputs) && inputs.length > 0 && inputs.length <= 10);
  assert.deepEqual(Object.keys(expected).sort(), Object.values(suites).sort());
  const reports = inputs.map(validateReport), runtime = reports[0].runtime;
  const groups = Object.fromEntries(Object.values(suites).map(s => [s, []]));
  for (const report of reports) {
    assert.equal(report.run_scope, "full_corpus");
    assert.deepEqual(report.runtime, runtime);
    const suite = suites[report.manifests[0].quality_version];
    assert.ok(suite);
    assert.deepEqual(report.manifests, expected[suite], "not the current source/annotation/request");
    assert.equal(report.manifests[0].execution_profile, "local-rss-v4");
    groups[suite].push(report);
  }
  const ordered = [...reports].sort((a, b) => Date.parse(a.started_at) - Date.parse(b.started_at));
  for (let i = 1; i < ordered.length; i++) {
    assert.ok(Date.parse(ordered[i].started_at) > Date.parse(ordered[i - 1].started_at));
    assert.ok(Date.parse(ordered[i].started_at) >= Date.parse(ordered[i - 1].ended_at));
  }
  const results = Object.entries(groups).map(([suite, runs]) => {
    const comparison = runs.length >= 2 ? compareReports(runs) : null;
    return { suite, required_runs: 2, supplied_runs: runs.length,
      passed: comparison !== null && comparison.all_passed && comparison.repeated_identically,
      comparison, diagnostics: diagnostics(runs, expected[suite]) };
  });
  return { schema: "rss-public-material-gate-v1", synthetic_only: true,
    material_origin: "public_document_paraphrase", annotation_method: "agent_prelabelled_unreviewed",
    execution_profile: "local-rss-v4", runtime, public_fixture_gate_passed: results.every(r => r.passed),
    suites: results, configuration_changed: false, human_reviewed: false,
    real_material_quality_verified: false, gaming_reserve_verified: false };
}
