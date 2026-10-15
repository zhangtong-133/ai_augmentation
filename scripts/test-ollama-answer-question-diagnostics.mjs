import { test } from "node:test";
import assert from "node:assert/strict";
import { verifyQuestionDiagnostic } from "./ollama-answer-question-diagnostics.mjs";
import { runCases } from "./local-answer-benchmark-lib.mjs";
const manifest = { case: { id: "fixture", expected_status: "answered" }, request_sha256: "a".repeat(64) };
const target = { endpoint: "http://127.0.0.1:11434", model_alias: "fixture" };
function fixture() {
  return { manifest, endpoint: target.endpoint, model: target.model_alias, synthetic_only: true, protocol_valid: true, elapsed_ms: 1,
    evaluation: { citation_valid: true, quality_pass: true, checks: ["expected_status", "required_terms", "expected_citations", "forbidden_terms"].map(name => ({ name, passed: true })) },
    semantic_diagnostic: { schema: "ollama-answer-question-diagnostic-v1", planned_requirements: 1, verified_requirements: 1,
      requirements_agree: true, requirements_supported: true, verified_excerpts: 1, planned_by_kind: { value: 0, availability: 1, fact: 0 },
      checks_by_kind: { value: { stated: 0, unavailable: 0, unsupported: 0 }, availability: { stated: 0, unavailable: 1, unsupported: 0 }, fact: { stated: 0, unavailable: 0, unsupported: 0 } } } };
}
test("question-only availability counts accept explicit absence while count disagreement is semantic abstention", async () => {
  const good = fixture(); assert.equal(verifyQuestionDiagnostic(good, manifest, target).quality_pass, true);
  const missing = fixture();
  Object.assign(missing.semantic_diagnostic, { verified_requirements: 2, requirements_agree: false, requirements_supported: false, checks_by_kind: null });
  missing.evaluation.checks[0].passed = false; missing.evaluation.quality_pass = false;
  const result = await runCases([manifest], target, async () => missing, verifyQuestionDiagnostic);
  assert.equal(result.complete, true); assert.equal(result.exit_code, 2);
  assert.equal(result.results[0].semantic_diagnostic.checks_by_kind, null);
});
test("question diagnostic rejects invented kind mappings, raw output, partial histograms and contradictory totals", async () => {
  for (const mutate of [
    raw => raw.semantic_diagnostic.keys = ["secret"],
    raw => raw.semantic_diagnostic.kinds = ["availability"],
    raw => raw.semantic_diagnostic.planned_by_kind.value = 1,
    raw => raw.semantic_diagnostic.verified_requirements = 2,
    raw => raw.semantic_diagnostic.planned_requirements = 13,
    raw => raw.semantic_diagnostic.verified_excerpts = 0,
    raw => raw.semantic_diagnostic.checks_by_kind = null,
    raw => raw.semantic_diagnostic.checks_by_kind.availability.unavailable = 0,
    raw => raw.semantic_diagnostic.checks_by_kind.fact.stated = 1,
    raw => raw.semantic_diagnostic.requirements_supported = false,
    raw => raw.semantic_diagnostic.planned_by_kind.availability = "secret",
    raw => raw.evaluation.answer = "secret",
  ]) {
    const raw = fixture(); mutate(raw);
    const result = await runCases([manifest], target, async () => raw, verifyQuestionDiagnostic);
    assert.equal(result.complete, false); assert.deepEqual(result.results, []);
    assert.ok(!JSON.stringify(result).includes("secret"));
  }
});
