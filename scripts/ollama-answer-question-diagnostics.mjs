import assert from "node:assert/strict";
import { verifyCase } from "./local-answer-benchmark-lib.mjs";

// Question-only types and source checks share only their ordinal positions, never model text.
export function verifyQuestionDiagnostic(raw, manifest, target) {
  const { semantic_diagnostic: value, ...ordinary } = raw;
  const result = verifyCase(ordinary, manifest, target);
  assert.deepEqual(Object.keys(value).sort(), ["checks_by_kind", "planned_by_kind", "planned_requirements", "requirements_agree", "requirements_supported", "schema", "verified_excerpts", "verified_requirements"]);
  assert.equal(value.schema, "ollama-answer-question-diagnostic-v1");
  for (const field of ["planned_requirements", "verified_requirements", "verified_excerpts"]) {
    assert.ok(Number.isSafeInteger(value[field]) && value[field] >= (field === "verified_excerpts" ? 0 : 1) && value[field] <= (field === "verified_excerpts" ? 64 : 12));
  }
  assert.equal(typeof value.requirements_agree, "boolean");
  assert.equal(typeof value.requirements_supported, "boolean");
  assert.equal(value.requirements_agree, value.planned_requirements === value.verified_requirements);
  assert.deepEqual(Object.keys(value.planned_by_kind).sort(), ["availability", "fact", "value"]);
  const planned = {}, counts = {}; let total = 0, unsupported = 0, nonempty = 0;
  for (const kind of ["value", "availability", "fact"]) {
    const count = value.planned_by_kind[kind];
    assert.ok(Number.isSafeInteger(count) && count >= 0 && count <= 12);
    planned[kind] = count; total += count;
  }
  assert.equal(total, value.planned_requirements);
  if (value.requirements_agree) {
    assert.deepEqual(Object.keys(value.checks_by_kind).sort(), ["availability", "fact", "value"]);
    for (const kind of ["value", "availability", "fact"]) {
      const group = value.checks_by_kind[kind]; let subtotal = 0; counts[kind] = {};
      assert.deepEqual(Object.keys(group).sort(), ["stated", "unavailable", "unsupported"]);
      for (const support of ["stated", "unavailable", "unsupported"]) {
        const count = group[support];
        assert.ok(Number.isSafeInteger(count) && count >= 0 && count <= 12);
        counts[kind][support] = count; subtotal += count;
        if (support === "unsupported" || support === "unavailable" && kind !== "availability") unsupported += count;
        if (support !== "unsupported") nonempty += count;
      }
      assert.equal(subtotal, planned[kind]);
    }
    assert.equal(value.requirements_supported, unsupported === 0);
    if (nonempty > 0) assert.ok(value.verified_excerpts > 0);
  } else {
    assert.equal(value.checks_by_kind, null);
    assert.equal(value.requirements_supported, false);
  }
  if (result.citation_valid) assert.equal(result.checks[0].passed,
    (value.requirements_supported ? "answered" : "insufficient_evidence") === manifest.case.expected_status);
  return { ...result, semantic_diagnostic: {
    schema: value.schema, planned_requirements: value.planned_requirements, verified_requirements: value.verified_requirements,
    requirements_agree: value.requirements_agree, requirements_supported: value.requirements_supported,
    verified_excerpts: value.verified_excerpts, planned_by_kind: planned, checks_by_kind: value.requirements_agree ? counts : null,
  } };
}
