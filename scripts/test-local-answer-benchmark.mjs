import { test } from "node:test";
import assert from "node:assert/strict";
import { verifyCase, runCases } from "./local-answer-benchmark-lib.mjs";
const target = { endpoint: "http://127.0.0.1:11435", model_alias: "fixed" };
const manifest = { case: { id: "single_fact", corpus_sha256: "a".repeat(64) }, request_sha256: "b".repeat(64) };
function output(m = manifest) {
  return { manifest: structuredClone(m), endpoint: target.endpoint, model: target.model_alias, synthetic_only: true, elapsed_ms: 1, protocol_valid: true,
    evaluation: { citation_valid: true, quality_pass: true, checks: ["expected_status", "required_terms", "expected_citations", "forbidden_terms"].map(name => ({ name, passed: true })) } };
}
test("changed content, target, extra model text and contradictory checks are rejected", () => {
  assert.equal(verifyCase(output(), manifest, target).quality_pass, true);
  for (const change of [r => r.manifest.request_sha256 = "x", r => r.manifest.case.corpus_sha256 = "x", r => r.model = "other", r => r.endpoint = "http://127.0.0.1:1", r => r.answer = "secret", r => r.evaluation.checks.reverse(), r => r.evaluation.citation_valid = false, r => r.evaluation.quality_pass = false, r => r.elapsed_ms = -1, r => r.protocol_valid = false]) {
    const raw = output(); change(raw); assert.throws(() => verifyCase(raw, manifest, target));
  }
});
test("quality failures continue once, protocol failure stops and marks remaining cases unrun", async () => {
  const manifests = [manifest, { ...manifest, case: { ...manifest.case, id: "second" } }, { ...manifest, case: { ...manifest.case, id: "third" } }];
  const sent = [];
  const report = await runCases(manifests, target, async id => {
    sent.push(id); const raw = output(manifests.find(m => m.case.id === id));
    if (id === "second") { raw.evaluation.checks[1].passed = false; raw.evaluation.quality_pass = false; }
    return raw;
  });
  assert.deepEqual(sent, ["single_fact", "second", "third"]); assert.equal(report.exit_code, 2); assert.equal(report.complete, true);
  const failed = await runCases(manifests, target, async () => { throw Object.assign(new Error("secret"), { answerFailure: "protocol" }); });
  assert.equal(failed.exit_code, 1); assert.equal(failed.failure, "protocol"); assert.deepEqual(failed.not_run, ["second", "third"]);
  assert.ok(!JSON.stringify(failed).includes("secret"));
});
test("citation failures cannot masquerade as content checks that passed", () => {
  const raw = output(); raw.evaluation.citation_valid = false; raw.evaluation.quality_pass = false;
  assert.throws(() => verifyCase(raw, manifest, target));
  raw.evaluation.checks.forEach(c => { c.passed = false; });
  assert.equal(verifyCase(raw, manifest, target).citation_valid, false);
});
