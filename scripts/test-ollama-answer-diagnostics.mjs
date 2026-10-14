import { test } from "node:test";
import assert from "node:assert/strict";
import { runCases, verifyCase } from "./local-answer-benchmark-lib.mjs";
import { verifyDiagnostic } from "./ollama-answer-diagnostics.mjs";

const manifest = { execution_profile: "ollama-knowledge-answer-v15-reuse", request_sha256: "a".repeat(64), case: { id: "fixture", expected_status: "answered" } };
const target = { endpoint: "http://127.0.0.1:11434", model_alias: "fixture" };
function fixture(agrees = true, supported = true) {
  const passed = agrees && supported;
  return { manifest, endpoint: target.endpoint, model: target.model_alias, synthetic_only: true, protocol_valid: true, elapsed_ms: 1,
    evaluation: { citation_valid: true, quality_pass: passed, checks: ["expected_status", "required_terms", "expected_citations", "forbidden_terms"].map(name => ({ name, passed: name === "forbidden_terms" || passed })) },
    semantic_diagnostic: { schema: "ollama-answer-semantic-diagnostic-v1", selected_excerpts: 1, verified_excerpts: 1,
      requirements: 1, evidence_agrees: agrees, requirements_supported: supported,
      checks_by_kind: { value: { stated: supported ? 1 : 0, unavailable: supported ? 0 : 1, unsupported: 0 },
        availability: { stated: 0, unavailable: 0, unsupported: 0 }, fact: { stated: 0, unavailable: 0, unsupported: 0 } } } };
}
test("diagnostics retain independent evidence agreement and requirement support without changing quality", async () => {
  for (const agrees of [true, false]) for (const supported of [true, false]) {
    const raw = fixture(agrees, supported);
    const output = await runCases([manifest], target, async () => raw, verifyDiagnostic);
    assert.equal(output.complete, true);
    assert.equal(output.exit_code, agrees && supported ? 0 : 2);
    assert.deepEqual(output.results[0].semantic_diagnostic, raw.semantic_diagnostic);
    assert.throws(() => verifyCase(raw, manifest, target));
    const { semantic_diagnostic, ...ordinary } = raw;
    assert.deepEqual(output.results[0], { ...verifyCase(ordinary, manifest, target), semantic_diagnostic });
  }
});
test("only availability accepts explicit absence; empty unsupported evidence remains a valid diagnostic", () => {
  const raw = fixture();
  raw.semantic_diagnostic.checks_by_kind.value.stated = 0;
  raw.semantic_diagnostic.checks_by_kind.availability.unavailable = 1;
  assert.equal(verifyDiagnostic(raw, manifest, target).quality_pass, true);
  const empty = fixture(true, false);
  Object.assign(empty.semantic_diagnostic, { selected_excerpts: 0, verified_excerpts: 0 });
  Object.assign(empty.semantic_diagnostic.checks_by_kind.value, { unavailable: 0, unsupported: 1 });
  assert.equal(verifyDiagnostic(empty, manifest, target).quality_pass, false);
});
test("arbitrary model content, missing metadata, inconsistent decisions and unbounded counters are rejected", async () => {
  for (const mutate of [
    raw => delete raw.semantic_diagnostic,
    raw => raw.answer = "secret",
    raw => raw.semantic_diagnostic.keys = ["s1u1"],
    raw => raw.semantic_diagnostic.schema = "secret",
    raw => raw.semantic_diagnostic.requirements = 0,
    raw => raw.semantic_diagnostic.requirements = 13,
    raw => raw.semantic_diagnostic.selected_excerpts = 65,
    raw => raw.semantic_diagnostic.selected_excerpts = 0,
    raw => raw.semantic_diagnostic.verified_excerpts = 0,
    raw => raw.semantic_diagnostic.requirements_supported = false,
    raw => raw.semantic_diagnostic.evidence_agrees = "true",
    raw => raw.semantic_diagnostic.checks_by_kind.value.stated = -1,
    raw => raw.semantic_diagnostic.checks_by_kind.value.stated = 1.5,
    raw => raw.semantic_diagnostic.checks_by_kind.value.stated = "secret",
    raw => raw.semantic_diagnostic.checks_by_kind.value.quote = "secret",
    raw => raw.semantic_diagnostic.checks_by_kind.value.unsupported = 1,
    raw => raw.semantic_diagnostic.checks_by_kind.extra = {},
    raw => raw.evaluation.checks[0].passed = false,
  ]) {
    const raw = fixture(); mutate(raw);
    assert.throws(() => verifyDiagnostic(raw, manifest, target));
    const report = await runCases([manifest], target, async () => raw, verifyDiagnostic);
    assert.equal(report.complete, false); assert.deepEqual(report.results, []);
    assert.ok(!JSON.stringify(report).includes("secret"));
  }
});
