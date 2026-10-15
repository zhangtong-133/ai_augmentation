import { test } from "node:test";
import assert from "node:assert/strict";
import { verifyGuardedDiagnostic } from "./ollama-answer-guarded-diagnostics.mjs";
import { runCases } from "./local-answer-benchmark-lib.mjs";
const manifest = { case: { id: "fixture", expected_status: "answered" }, request_sha256: "a".repeat(64) };
const target = { endpoint: "http://127.0.0.1:11434", model_alias: "fixture" };
function questionFixture() {
  return { manifest, endpoint: target.endpoint, model: target.model_alias, synthetic_only: true, protocol_valid: true, elapsed_ms: 1,
    evaluation: { citation_valid: true, quality_pass: true, checks: ["expected_status", "required_terms", "expected_citations", "forbidden_terms"].map(name => ({ name, passed: true })) },
    semantic_diagnostic: { schema: "ollama-answer-question-diagnostic-v1", planned_requirements: 1, verified_requirements: 1,
      requirements_agree: true, requirements_supported: true, verified_excerpts: 1, planned_by_kind: { value: 0, availability: 1, fact: 0 },
      checks_by_kind: { value: { stated: 0, unavailable: 0, unsupported: 0 }, availability: { stated: 0, unavailable: 1, unsupported: 0 }, fact: { stated: 0, unavailable: 0, unsupported: 0 } } } };
}
function fixture() {
  const raw = questionFixture();
  raw.semantic_diagnostic = { schema:"ollama-answer-guarded-diagnostic-v1", selected_excerpts:1, evidence_agrees:true, verifier_kind_disagreements:1, question:raw.semantic_diagnostic };
  return raw;
}
test("guarded diagnostics combine question support and independent evidence while allowing kind disagreements", () => {
  assert.equal(verifyGuardedDiagnostic(fixture(),manifest,target).quality_pass,true);
  const raw=fixture();raw.semantic_diagnostic.evidence_agrees=false;
  raw.evaluation.checks[0].passed=false;raw.evaluation.quality_pass=false;
  assert.equal(verifyGuardedDiagnostic(raw,manifest,target).quality_pass,false);
  const mismatch=fixture();Object.assign(mismatch.semantic_diagnostic.question,{verified_requirements:2,requirements_agree:false,requirements_supported:false,checks_by_kind:null});
  mismatch.semantic_diagnostic.verifier_kind_disagreements=null;
  mismatch.evaluation.checks[0].passed=false;mismatch.evaluation.quality_pass=false;
  assert.equal(verifyGuardedDiagnostic(mismatch,manifest,target).semantic_diagnostic.verifier_kind_disagreements,null);
});
test("guarded diagnostics reject raw fields, wrong counts and contradictory final states", async () => {
  for(const mutate of [
    raw=>raw.semantic_diagnostic.keys=["secret"],
    raw=>raw.semantic_diagnostic.question.answer="secret",
    raw=>raw.semantic_diagnostic.selected_excerpts=65,
    raw=>raw.semantic_diagnostic.selected_excerpts=0,
    raw=>raw.semantic_diagnostic.verifier_kind_disagreements=2,
    raw=>raw.semantic_diagnostic.verifier_kind_disagreements=null,
    raw=>raw.semantic_diagnostic.evidence_agrees=false,
    raw=>raw.semantic_diagnostic.question.requirements_supported=false,
    raw=>raw.semantic_diagnostic.schema="ollama-answer-question-diagnostic-v1",
  ]) {
    const raw=fixture();mutate(raw);const result=await runCases([manifest],target,async()=>raw,verifyGuardedDiagnostic);
    assert.equal(result.complete,false);assert.deepEqual(result.results,[]);assert.ok(!JSON.stringify(result).includes("secret"));
  }
});
