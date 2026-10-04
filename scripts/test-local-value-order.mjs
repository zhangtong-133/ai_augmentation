import { test } from "node:test";
import assert from "node:assert/strict";
import { auditOrder } from "./local-value-order-lib.mjs";
import { report } from "./quality-test-fixture.mjs";
function fixtures() {
  const reports = [report(), report("2026-10-04T12:10:00.000Z")];
  reports.forEach(r => r.manifests.forEach(m => m.quality_version = "rss-order-v1"));
  return { reports, expected: structuredClone(reports[0].manifests) };
}
test("order audit requires passing conditions plus within-round and across-round invariance", () => {
  const { reports, expected } = fixtures();
  const audit = auditOrder(reports, expected);
  assert.equal(audit.order_gate_passed, true);
  assert.equal(audit.configuration_changed, false);
  assert.equal(audit.real_material_quality_verified, false);
  reports[0].results[0].scores.article = 81; // still quality-passing, but order-sensitive
  const changed = auditOrder(reports, expected);
  assert.equal(changed.comparison.all_passed, true);
  assert.equal(changed.order_gate_passed, false);
  assert.equal(changed.rounds[0].order_invariant, false);
  reports[0].results.forEach(r => r.scores.article = 81);
  const drift = auditOrder(reports, expected);
  assert.ok(drift.rounds.every(r => r.order_invariant));
  assert.equal(drift.order_gate_passed, false); // stable per round, changed between rounds
});
test("uniform abstention fails required scoring and remains visible in coverage", () => {
  const { reports, expected } = fixtures();
  reports.forEach(r => {
    r.results.forEach(result => { result.scores.article = null; result.quality_pass = false; result.checks[0].passed = false; });
    r.passed_cases = 0; r.exit_code = 2;
  });
  const audit = auditOrder(reports, expected);
  assert.ok(audit.rounds.every(r => r.order_invariant));
  assert.equal(audit.order_gate_passed, false);
  assert.equal(audit.comparison.coverage.groups.required_score.abstention_rate, 1);
});
test("partial protocol failure leaves unknown invariance and preserves missing permutations", () => {
  const { reports, expected } = fixtures(), failed = reports[1];
  failed.complete = false; failed.results = failed.results.slice(0, 1); failed.passed_cases = 1;
  failed.exit_code = 1; failed.failed_case = "b"; failed.failure = "transport";
  const audit = auditOrder(reports, expected);
  assert.equal(audit.order_gate_passed, false); assert.equal(audit.rounds[1].order_invariant, null);
  assert.equal(audit.rounds[1].items[0].completed_permutations, 1);
  assert.equal(audit.comparison.coverage.total.protocol_failed, 1);
  assert.equal(audit.comparison.coverage.total.not_run, 1);
});
test("duplicate, overlapping, altered conditions and other suites cannot count as order evidence", () => {
  for (const mutate of [r => r[1] = structuredClone(r[0]), r => r[0].ended_at = "2026-10-04T12:11:00.000Z",
    r => r[0].runtime.model_sha256 = "3".repeat(64), r => r.forEach(x => x.manifests[0].request_sha256 = "4".repeat(64)),
    r => r.forEach(x => x.manifests.forEach(m => m.quality_version = "rss-quality-v1")), r => delete r[0].run_scope]) {
    const { reports, expected } = fixtures(); mutate(reports); assert.throws(() => auditOrder(reports, expected));
  }
});

test("invariance aligns semantic labels when numeric model ids change with each permutation", () => {
  const { reports } = fixtures();
  for (const r of reports) {
    r.manifests.forEach((m, i) => {
      m.items = i % 2 ? ["weather", "article"] : ["article", "weather"];
      m.checks = [{ kind: "minimum", item: "article", value: 60 }, { kind: "minimum", item: "weather", value: 0 }, { kind: "ceiling", item: "weather", value: 20 }];
    });
    r.results.forEach(result => { result.scores.weather = 0; result.checks.push({ index: 1, passed: true }, { index: 2, passed: true }); });
  }
  const audit = auditOrder(reports, structuredClone(reports[0].manifests));
  assert.equal(audit.order_gate_passed, true);
  assert.deepEqual(audit.rounds[0].items.map(i => [i.item, i.distinct_scores]), [["article", [80]], ["weather", [0]]]);
  assert.equal(audit.comparison.coverage.groups.required_score.scored, 12);
});
