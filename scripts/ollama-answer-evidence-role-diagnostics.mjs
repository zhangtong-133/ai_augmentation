import assert from "node:assert/strict";
import { verifyGuardedDiagnostic } from "./ollama-answer-guarded-diagnostics.mjs";

const supports = {
  asserted_fact: "stated",
  explicit_absence: "unavailable",
  instruction: "unsupported",
  unrelated: "unsupported",
  missing: "unsupported",
};

// The role is a bounded verifier judgment, not independent proof of semantic correctness.
export function verifyEvidenceRoleDiagnostic(raw, manifest, target) {
  const { semantic_diagnostic: value, ...ordinary } = raw;
  assert.deepEqual(Object.keys(value).sort(), ["basis", "guarded", "schema"]);
  assert.equal(value.schema, "ollama-answer-evidence-role-diagnostic-v1");
  assert.equal(value.guarded.schema, "ollama-answer-guarded-diagnostic-v2");
  const { semantic_diagnostic: guarded, ...result } = verifyGuardedDiagnostic(
    { ...ordinary, semantic_diagnostic: value.guarded }, manifest, target);
  assert.ok(Array.isArray(value.basis) && value.basis.length >= 1 && value.basis.length <= 12);
  assert.equal(value.basis.length, guarded.trace.checks.length);
  const basis = value.basis.map((role, index) => {
    assert.equal(typeof role, "string");
    assert.ok(Object.hasOwn(supports, role));
    assert.equal(supports[role], guarded.trace.checks[index].support);
    return role;
  });
  return { ...result, semantic_diagnostic: { schema: value.schema, basis, guarded } };
}
