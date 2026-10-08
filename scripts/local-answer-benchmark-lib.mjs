import assert from "node:assert/strict";
const names = ["expected_status", "required_terms", "expected_citations", "forbidden_terms"];
export function verifyCase(raw, manifest, target) {
  assert.deepEqual(Object.keys(raw).sort(), ["elapsed_ms", "endpoint", "evaluation", "manifest", "model", "protocol_valid", "synthetic_only"]);
  assert.deepEqual(raw.manifest, manifest);
  assert.equal(raw.endpoint, target.endpoint); assert.equal(raw.model, target.model_alias);
  assert.equal(raw.synthetic_only, true); assert.equal(raw.protocol_valid, true);
  assert.ok(Number.isSafeInteger(raw.elapsed_ms) && raw.elapsed_ms >= 0 && raw.elapsed_ms <= 90000);
  const value = raw.evaluation;
  assert.deepEqual(Object.keys(value).sort(), ["checks", "citation_valid", "quality_pass"]);
  assert.equal(typeof value.citation_valid, "boolean");
  assert.equal(value.checks.length, names.length);
  value.checks.forEach((check, i) => {
    assert.deepEqual(Object.keys(check).sort(), ["name", "passed"]);
    assert.equal(check.name, names[i]); assert.equal(typeof check.passed, "boolean");
    if (!value.citation_valid) assert.equal(check.passed, false);
  });
  assert.equal(value.quality_pass, value.citation_valid && value.checks.every(c => c.passed));
  return { id: manifest.case.id, request_sha256: manifest.request_sha256, elapsed_ms: raw.elapsed_ms,
    protocol_valid: true, citation_valid: value.citation_valid, quality_pass: value.quality_pass,
    checks: value.checks.map(c => ({ name: c.name, passed: c.passed })) };
}
export async function runCases(manifests, target, send) {
  const results = [];
  for (const manifest of manifests) {
    try { results.push(verifyCase(await send(manifest.case.id), manifest, target)); }
    catch (error) {
      return { complete: false, failed_case: manifest.case.id,
        failure: ["transport", "protocol", "resource", "runtime"].includes(error.answerFailure) ? error.answerFailure : "case_unconfirmed",
        not_run: manifests.slice(results.length + 1).map(m => m.case.id), results, exit_code: 1 };
    }
  }
  return { complete: true, not_run: [], results, exit_code: results.every(r => r.quality_pass) ? 0 : 2 };
}
