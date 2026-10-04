import { test } from "node:test";
import assert from "node:assert/strict";
import { publicGate } from "./local-value-public-lib.mjs";
import { report } from "./quality-test-fixture.mjs";
function fixtures() {
  const reports = [0, 1, 2, 3].map(i => report(`2026-10-04T12:0${i}:00.000Z`));
  reports.forEach((r, index) => r.manifests.forEach(m => {
    m.quality_version = index < 2 ? "rss-public-calibration-v1" : "rss-public-holdout-v1";
    m.material_origin = "public_document_paraphrase"; m.execution_profile = "local-rss-v4";
  }));
  return { reports, expected: { public_calibration: structuredClone(reports[0].manifests), public_holdout: structuredClone(reports[2].manifests) } };
}
test("two independent complete rounds per split pass only as unreviewed fixed fixtures", () => {
  const { reports, expected } = fixtures(), result = publicGate(reports, expected);
  assert.equal(result.public_fixture_gate_passed, true); assert.equal(result.human_reviewed, false);
  assert.equal(result.configuration_changed, false); assert.equal(result.real_material_quality_verified, false);
  assert.equal(publicGate(reports.slice(0, 2), expected).public_fixture_gate_passed, false);
  assert.equal(publicGate(reports.slice(1), expected).public_fixture_gate_passed, false);
  assert.equal(result.suites[0].diagnostics[0].required_score_rate, 1);
});
test("partial high, low and abstention errors stay separate and cannot pass the gate", () => {
  for (const score of [80, 0, null, 40]) {
    const { reports } = fixtures();
    for (const r of reports) {
      r.manifests[0].checks = [{ kind: "minimum", item: "article", value: 40 }, { kind: "ceiling", item: "article", value: 40 }];
      r.results[0].scores.article = score;
      r.results[0].checks = [{ index: 0, passed: score !== null && score >= 40 }, { index: 1, passed: score === null || score <= 40 }];
      r.results[0].quality_pass = score === 40; r.passed_cases = score === 40 ? 3 : 2; r.exit_code = score === 40 ? 0 : 2;
    }
    const expected = { public_calibration: structuredClone(reports[0].manifests), public_holdout: structuredClone(reports[2].manifests) };
    const result = publicGate(reports, expected), d = result.suites[0].diagnostics[0];
    assert.equal(result.public_fixture_gate_passed, score === 40);
    assert.equal(d.partial_overrated, score === 80 ? 2 : 0);
    assert.equal(d.partial_underrated, score === 0 ? 2 : 0);
    assert.equal(d.partial_abstained, score === null ? 2 : 0);
    assert.equal(d.partial_correct, score === 40 ? 2 : 0);
    assert.equal(d.above_ceiling, score === 80 ? 2 : 0);
    assert.equal(d.below_minimum, score === 0 ? 2 : 0);
    assert.equal(d.unexpected_abstentions, score === null ? 2 : 0);
  }
});
test("protocol failure and not-run do not become abstention or inflate score rate", () => {
  const { reports, expected } = fixtures();
  for (const r of reports.slice(0, 2)) {
    r.complete = false; r.results = []; r.passed_cases = 0; r.exit_code = 1; r.failed_case = "a"; r.failure = "transport";
  }
  const result = publicGate(reports, expected), [first, second] = result.suites[0].diagnostics;
  assert.equal(result.public_fixture_gate_passed, false); assert.equal(first.failed_case_items, 2);
  assert.equal(second.not_run_items, 2); assert.equal(first.required_score_rate, null);
  assert.equal(first.abstained_items, 0); assert.equal(first.completion_rate, 0);
});
test("extra good runs cannot erase a failed round or changes to sources, labels or runtime", () => {
  const { reports, expected } = fixtures();
  const extra = structuredClone(reports[0]); extra.started_at = extra.ended_at = "2026-10-04T12:04:00.000Z"; reports.push(extra);
  reports[0].results[0].scores.article = null; reports[0].results[0].checks[0].passed = false;
  reports[0].results[0].quality_pass = false; reports[0].passed_cases = 2; reports[0].exit_code = 2;
  assert.equal(publicGate(reports, expected).public_fixture_gate_passed, false);
  for (const mutate of [r => r[1] = structuredClone(r[0]), r => r[0].ended_at = r[2].started_at,
    r => r[0].runtime.model_sha256 = "f".repeat(64), r => r.forEach(x => x.manifests[0].corpus_sha256 = "f".repeat(64)),
    r => r.forEach(x => x.manifests[0].checks[0].value = 50), r => r[0].manifests[0].material_origin = "human_verified"]) {
    const fixture = fixtures(); mutate(fixture.reports); assert.throws(() => publicGate(fixture.reports, fixture.expected));
  }
});

test("quality-passing but changing scores cannot pass as stable repeated evidence", () => {
  const { reports } = fixtures();
  reports.forEach(r => r.manifests.forEach(m => m.checks[0].value = 0));
  const expected = { public_calibration: structuredClone(reports[0].manifests), public_holdout: structuredClone(reports[2].manifests) };
  reports[1].results[0].scores.article = 40;
  const result = publicGate(reports, expected);
  assert.equal(result.suites[0].comparison.all_passed, true);
  assert.equal(result.suites[0].comparison.repeated_identically, false);
  assert.equal(result.public_fixture_gate_passed, false);
});
