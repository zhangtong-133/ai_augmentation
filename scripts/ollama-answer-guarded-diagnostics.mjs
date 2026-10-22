import assert from "node:assert/strict";
import { verifyCase } from "./local-answer-benchmark-lib.mjs";
import { validateQuestionDiagnostic } from "./ollama-answer-question-diagnostics.mjs";

const kinds = ["value", "availability", "fact"];
const supports = ["stated", "unavailable", "unsupported"];

function validatePositions(value, catalogSize) {
  assert.ok(Array.isArray(value) && value.length <= catalogSize);
  let previous = 0;
  for (const position of value) {
    assert.ok(Number.isSafeInteger(position) && position > previous && position <= catalogSize);
    previous = position;
  }
  return [...value];
}

function validateTrace(value, diagnostic, question) {
  assert.deepEqual(Object.keys(value).sort(), ["catalog_size", "checks", "planned_kinds", "selected_positions"]);
  assert.ok(Number.isSafeInteger(value.catalog_size) && value.catalog_size >= 1 && value.catalog_size <= 64);
  const selected = validatePositions(value.selected_positions, value.catalog_size);
  assert.ok(Array.isArray(value.planned_kinds) && value.planned_kinds.length >= 1 && value.planned_kinds.length <= 12);
  const planned = value.planned_kinds.map(kind => { assert.ok(kinds.includes(kind)); return kind; });
  assert.ok(Array.isArray(value.checks) && value.checks.length >= 1 && value.checks.length <= 12);
  const checks = value.checks.map(check => {
    assert.deepEqual(Object.keys(check).sort(), ["evidence_positions", "kind", "support"]);
    assert.ok(kinds.includes(check.kind));
    assert.ok(supports.includes(check.support));
    const evidence = validatePositions(check.evidence_positions, value.catalog_size);
    if (check.support !== "unsupported") assert.ok(evidence.length > 0);
    return { kind: check.kind, support: check.support, evidence_positions: evidence };
  });
  const verified = [...new Set(checks.flatMap(check => check.evidence_positions))].sort((a, b) => a - b);
  const agrees = planned.length === checks.length;
  const plannedCounts = Object.fromEntries(kinds.map(kind => [kind, planned.filter(item => item === kind).length]));
  const checkCounts = agrees ? Object.fromEntries(kinds.map(kind => [kind,
    Object.fromEntries(supports.map(support => [support, checks.filter((check, index) => planned[index] === kind && check.support === support).length]))])) : null;
  assert.equal(diagnostic.selected_excerpts, selected.length);
  assert.equal(diagnostic.evidence_agrees, selected.length === verified.length && selected.every((position, index) => position === verified[index]));
  assert.equal(diagnostic.verifier_kind_disagreements, agrees ? checks.filter((check, index) => check.kind !== planned[index]).length : null);
  assert.equal(question.planned_requirements, planned.length);
  assert.equal(question.verified_requirements, checks.length);
  assert.equal(question.requirements_agree, agrees);
  assert.equal(question.requirements_supported, agrees && checks.every((check, index) => check.support === "stated" || check.support === "unavailable" && planned[index] === "availability"));
  assert.equal(question.verified_excerpts, verified.length);
  assert.deepEqual(question.planned_by_kind, plannedCounts);
  assert.deepEqual(question.checks_by_kind, checkCounts);
  return { catalog_size: value.catalog_size, selected_positions: selected, planned_kinds: planned, checks };
}

export function verifyGuardedDiagnostic(raw, manifest, target) {
  const { semantic_diagnostic: value, ...ordinary } = raw;
  const result = verifyCase(ordinary, manifest, target);
  assert.ok(["ollama-answer-guarded-diagnostic-v1", "ollama-answer-guarded-diagnostic-v2"].includes(value.schema));
  const fields = ["evidence_agrees", "question", "schema", "selected_excerpts", "verifier_kind_disagreements"];
  if (value.schema === "ollama-answer-guarded-diagnostic-v2") fields.push("trace");
  assert.deepEqual(Object.keys(value).sort(), fields.sort());
  const question = validateQuestionDiagnostic(value.question);
  assert.ok(Number.isSafeInteger(value.selected_excerpts) && value.selected_excerpts >= 0 && value.selected_excerpts <= 64);
  assert.equal(typeof value.evidence_agrees, "boolean");
  if (value.evidence_agrees) assert.equal(value.selected_excerpts, question.verified_excerpts);
  else assert.ok(value.selected_excerpts + question.verified_excerpts > 0);
  if (question.requirements_agree) assert.ok(Number.isSafeInteger(value.verifier_kind_disagreements) && value.verifier_kind_disagreements >= 0 && value.verifier_kind_disagreements <= question.planned_requirements);
  else assert.equal(value.verifier_kind_disagreements, null);
  const trace = value.schema === "ollama-answer-guarded-diagnostic-v2" ? validateTrace(value.trace, value, question) : undefined;
  if (result.citation_valid) assert.equal(result.checks[0].passed,
    (value.evidence_agrees && question.requirements_supported ? "answered" : "insufficient_evidence") === manifest.case.expected_status);
  return { ...result, semantic_diagnostic: { schema: value.schema, selected_excerpts: value.selected_excerpts,
    evidence_agrees: value.evidence_agrees, verifier_kind_disagreements: value.verifier_kind_disagreements, question,
    ...(trace === undefined ? {} : { trace }) } };
}
