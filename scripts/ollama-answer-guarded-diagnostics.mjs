import assert from "node:assert/strict";
import { verifyCase } from "./local-answer-benchmark-lib.mjs";
import { validateQuestionDiagnostic } from "./ollama-answer-question-diagnostics.mjs";

export function verifyGuardedDiagnostic(raw, manifest, target) {
  const { semantic_diagnostic: value, ...ordinary } = raw;
  const result = verifyCase(ordinary, manifest, target);
  assert.deepEqual(Object.keys(value).sort(), ["evidence_agrees", "question", "schema", "selected_excerpts", "verifier_kind_disagreements"]);
  assert.equal(value.schema, "ollama-answer-guarded-diagnostic-v1");
  const question = validateQuestionDiagnostic(value.question);
  assert.ok(Number.isSafeInteger(value.selected_excerpts) && value.selected_excerpts >= 0 && value.selected_excerpts <= 64);
  assert.equal(typeof value.evidence_agrees, "boolean");
  if (value.evidence_agrees) assert.equal(value.selected_excerpts, question.verified_excerpts);
  else assert.ok(value.selected_excerpts + question.verified_excerpts > 0);
  if (question.requirements_agree) assert.ok(Number.isSafeInteger(value.verifier_kind_disagreements) && value.verifier_kind_disagreements >= 0 && value.verifier_kind_disagreements <= question.planned_requirements);
  else assert.equal(value.verifier_kind_disagreements, null);
  if (result.citation_valid) assert.equal(result.checks[0].passed,
    (value.evidence_agrees && question.requirements_supported ? "answered" : "insufficient_evidence") === manifest.case.expected_status);
  return { ...result, semantic_diagnostic: { schema: value.schema, selected_excerpts: value.selected_excerpts,
    evidence_agrees: value.evidence_agrees, verifier_kind_disagreements: value.verifier_kind_disagreements, question } };
}
