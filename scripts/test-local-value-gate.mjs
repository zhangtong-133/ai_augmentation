import { test } from "node:test";
import assert from "node:assert/strict";
import { qualityGate } from "./local-value-gate-lib.mjs";
import { report } from "./quality-test-fixture.mjs";
function fixtures() {
  const reports = [0, 1, 2, 3].map(i => report(`2026-10-04T12:0${i}:00.000Z`));
  reports.slice(2).forEach(r => r.manifests.forEach(m => { m.quality_version = "rss-challenge-v1"; m.corpus_sha256 = "9".repeat(64); }));
  return { reports, expected: { baseline: structuredClone(reports[0].manifests), challenge: structuredClone(reports[2].manifests) } };
}
test("both frozen suites require two passing rounds without enabling production", () => {
  const { reports, expected } = fixtures();
  const gate = qualityGate(reports, expected);
  assert.equal(gate.synthetic_gate_passed, true);
  assert.equal(gate.configuration_changed, false);
  assert.equal(gate.real_material_quality_verified, false);
  assert.equal(gate.gaming_reserve_verified, false);
  assert.equal(qualityGate(reports.slice(0, 2), expected).synthetic_gate_passed, false);
  assert.equal(qualityGate(reports.slice(1), expected).synthetic_gate_passed, false);
});
test("protocol and quality failures block the gate even with additional passing rounds", () => {
  for (const protocol of [true, false]) {
    const { reports, expected } = fixtures(), failed = reports[3];
    const extra = structuredClone(failed); extra.started_at = extra.ended_at = "2026-10-04T12:04:00.000Z"; reports.push(extra);
    if (protocol) {
      failed.complete = false; failed.results = []; failed.passed_cases = 0; failed.exit_code = 1;
      failed.failed_case = "a"; failed.failure = "output_reason_category";
    } else {
      failed.results[0].scores.article = 0; failed.results[0].checks[0].passed = false;
      failed.results[0].quality_pass = false; failed.passed_cases = 2; failed.exit_code = 2;
    }
    assert.equal(qualityGate(reports, expected).synthetic_gate_passed, false);
  }
});
test("copied or overlapping rounds cannot supply repeat evidence", () => {
  for (const mutate of [r => r[1] = structuredClone(r[0]), r => r[0].ended_at = r[2].started_at]) {
    const { reports, expected } = fixtures(); mutate(reports);
    assert.throws(() => qualityGate(reports, expected));
  }
});
test("tampered or obsolete manifests and different runtimes cannot pass as frozen conditions", () => {
  for (const mutate of [r => r[0].runtime.model_sha256 = "8".repeat(64),
    r => r.forEach(x => x.manifests.forEach(m => m.checks[0].value = 1)),
    r => r.forEach(x => x.manifests.forEach(m => m.execution_profile = "local-rss-v1")),
    r => { delete r[0].run_scope; }, r => r.forEach(x => { x.manifests.pop(); x.results.pop(); x.total_cases--; x.passed_cases--; })]) {
    const { reports, expected } = fixtures(); mutate(reports);
    assert.throws(() => qualityGate(reports, expected));
  }
});
