import assert from "node:assert/strict";
// Project only the verified metadata schema: never persist arbitrary child output.
export function verifyCase(raw, manifest, target) {
  assert.deepEqual(Object.keys(raw).sort(), ["elapsed_ms", "endpoint", "model", "response_bytes", "result", "synthetic_only"]);
  assert.equal(raw.endpoint, target.endpoint); assert.equal(raw.model, target.model_alias);
  assert.equal(raw.synthetic_only, true);
  assert.ok(Number.isSafeInteger(raw.elapsed_ms) && raw.elapsed_ms >= 0 && raw.elapsed_ms <= 90000);
  assert.ok(Number.isSafeInteger(raw.response_bytes) && raw.response_bytes > 0 && raw.response_bytes <= 32768);
  const r = raw.result;
  assert.deepEqual(Object.keys(r).sort(), ["checks", "manifest", "protocol_valid", "quality_pass", "scores"]);
  assert.deepEqual(r.manifest, manifest); assert.equal(r.protocol_valid, true);
  assert.deepEqual(Object.keys(r.scores).sort(), [...manifest.items].sort());
  for (const score of Object.values(r.scores)) assert.ok(score === null || (Number.isInteger(score) && score >= 0 && score <= 100));
  assert.equal(r.checks.length, manifest.checks.length);
  r.checks.forEach((check, index) => {
    assert.deepEqual(Object.keys(check).sort(), ["index", "passed"]);
    assert.equal(check.index, index); assert.equal(typeof check.passed, "boolean");
  });
  assert.equal(r.quality_pass, r.checks.every(c => c.passed));
  return { id: manifest.id, input_digest: manifest.input_digest, request_sha256: manifest.request_sha256,
    elapsed_ms: raw.elapsed_ms, response_bytes: raw.response_bytes, protocol_valid: true,
    quality_pass: r.quality_pass, checks: r.checks.map(c => ({ index: c.index, passed: c.passed })), scores: { ...r.scores } };
}
export async function runCases(manifests, target, send) {
  const results = [];
  for (const manifest of manifests) {
    try { results.push(verifyCase(await send(manifest.id), manifest, target)); }
    catch (error) {
      const known = ["transport", "output_json", "output_schema", "output_count", "output_ids", "output_score", "output_reason", "output_reason_category", "output_strict"];
      return { complete: false, failure: known.includes(error.benchmarkFailure) ? error.benchmarkFailure : "case_execution_or_protocol_unconfirmed", failed_case: manifest.id, results, exit_code: 1 };
    }
  }
  return { complete: true, results, exit_code: results.every(r => r.quality_pass) ? 0 : 2 };
}
