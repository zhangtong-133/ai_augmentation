import assert from "node:assert/strict";
import { verifyCase } from "./local-answer-benchmark-lib.mjs";

export const DIAGNOSTIC_REPORT_SCHEMA = "ollama-answer-semantic-report-v1";
// Allow only fixed counters and booleans. Never pass arbitrary child output to a report.
export function verifyDiagnostic(raw, manifest, target) {
  const { semantic_diagnostic: value, ...ordinary } = raw;
  const result = verifyCase(ordinary, manifest, target);
  assert.deepEqual(Object.keys(value).sort(), ["checks_by_kind", "evidence_agrees", "requirements", "requirements_supported", "schema", "selected_excerpts", "verified_excerpts"]);
  assert.equal(value.schema, "ollama-answer-semantic-diagnostic-v1");
  for (const field of ["selected_excerpts", "verified_excerpts", "requirements"]) {
    assert.ok(Number.isSafeInteger(value[field]) && value[field] >= 0 && value[field] <= (field === "requirements" ? 12 : 64));
  }
  assert.ok(value.requirements > 0);
  assert.equal(typeof value.evidence_agrees, "boolean");
  assert.equal(typeof value.requirements_supported, "boolean");
  assert.deepEqual(Object.keys(value.checks_by_kind).sort(), ["availability", "fact", "value"]);
  let total = 0, unsupported = 0, nonempty = 0;
  const counts = {};
  for (const kind of ["value", "availability", "fact"]) {
    const group = value.checks_by_kind[kind];
    assert.deepEqual(Object.keys(group).sort(), ["stated", "unavailable", "unsupported"]);
    counts[kind] = {};
    for (const support of ["stated", "unavailable", "unsupported"]) {
      const count = group[support];
      assert.ok(Number.isSafeInteger(count) && count >= 0 && count <= 12);
      counts[kind][support] = count; total += count;
      if (support === "unsupported" || support === "unavailable" && kind !== "availability") unsupported += count;
      if (support !== "unsupported") nonempty += count;
    }
  }
  assert.equal(total, value.requirements);
  assert.equal(value.requirements_supported, unsupported === 0);
  if (nonempty > 0) assert.ok(value.verified_excerpts > 0);
  if (value.evidence_agrees) assert.equal(value.selected_excerpts, value.verified_excerpts);
  if (value.selected_excerpts === 0 && value.verified_excerpts === 0) assert.equal(value.evidence_agrees, true);
  const answered = value.evidence_agrees && value.requirements_supported;
  if (result.citation_valid) {
    assert.equal(result.checks[0].passed, (answered ? "answered" : "insufficient_evidence") === manifest.case.expected_status);
  }
  return { ...result, semantic_diagnostic: {
    schema: value.schema, selected_excerpts: value.selected_excerpts, verified_excerpts: value.verified_excerpts,
    requirements: value.requirements, evidence_agrees: value.evidence_agrees,
    requirements_supported: value.requirements_supported, checks_by_kind: counts,
  } };
}
