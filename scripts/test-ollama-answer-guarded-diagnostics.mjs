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
function traceFixture() {
  const raw = fixture();
  Object.assign(raw.semantic_diagnostic, {
    schema: "ollama-answer-guarded-diagnostic-v2",
    trace: { catalog_size: 3, selected_positions: [1], planned_kinds: ["availability"],
      checks: [{ kind: "fact", support: "unavailable", evidence_positions: [1] }] },
  });
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
test("guarded v2 trace preserves ordinal evidence and uses planner kinds for support", () => {
  const raw = traceFixture();
  const diagnostic = raw.semantic_diagnostic;
  diagnostic.trace.planned_kinds.push("value");
  diagnostic.trace.checks.push({ kind: "availability", support: "stated", evidence_positions: [1] });
  Object.assign(diagnostic.question, { planned_requirements: 2, verified_requirements: 2,
    planned_by_kind: { value: 1, availability: 1, fact: 0 },
    checks_by_kind: { value: { stated: 1, unavailable: 0, unsupported: 0 }, availability: { stated: 0, unavailable: 1, unsupported: 0 }, fact: { stated: 0, unavailable: 0, unsupported: 0 } } });
  diagnostic.verifier_kind_disagreements = 2;
  const result = verifyGuardedDiagnostic(raw, manifest, target);
  assert.deepEqual(result.semantic_diagnostic, diagnostic);
  assert.notEqual(result.semantic_diagnostic.trace, diagnostic.trace);
  assert.notEqual(result.semantic_diagnostic.trace.checks[0].evidence_positions, diagnostic.trace.checks[0].evidence_positions);
  assert.equal(result.semantic_diagnostic.question.verified_excerpts, 1);
  assert.equal(result.quality_pass, true);
});
test("guarded v2 trace invalidates all kind pairing when requirement counts differ", () => {
  const raw = traceFixture();
  const diagnostic = raw.semantic_diagnostic;
  diagnostic.trace.selected_positions.push(2);
  diagnostic.trace.checks.push({ kind: "value", support: "stated", evidence_positions: [2] });
  Object.assign(diagnostic, { selected_excerpts: 2, verifier_kind_disagreements: null });
  Object.assign(diagnostic.question, { verified_requirements: 2, verified_excerpts: 2,
    requirements_agree: false, requirements_supported: false, checks_by_kind: null });
  raw.evaluation.checks[0].passed = false; raw.evaluation.quality_pass = false;
  const result = verifyGuardedDiagnostic(raw, manifest, target);
  assert.equal(result.semantic_diagnostic.verifier_kind_disagreements, null);
  assert.equal(result.semantic_diagnostic.question.checks_by_kind, null);
  assert.equal(result.semantic_diagnostic.trace.checks.length, 2);
  const inventedPairing = structuredClone(raw);
  inventedPairing.semantic_diagnostic.verifier_kind_disagreements = 1;
  assert.throws(() => verifyGuardedDiagnostic(inventedPairing, manifest, target));
});
test("guarded v2 trace permits empty unsupported evidence and both catalog boundaries", () => {
  for (const catalogSize of [1, 64]) {
    const raw = traceFixture();
    raw.semantic_diagnostic.trace.catalog_size = catalogSize;
    raw.semantic_diagnostic.trace.selected_positions = [catalogSize];
    raw.semantic_diagnostic.trace.checks[0].evidence_positions = [catalogSize];
    assert.equal(verifyGuardedDiagnostic(raw, manifest, target).quality_pass, true);
  }
  for (const evidence of [[], [1]]) {
    const raw = traceFixture();
    const diagnostic = raw.semantic_diagnostic;
    diagnostic.trace.selected_positions = [...evidence];
    diagnostic.trace.checks[0].support = "unsupported";
    diagnostic.trace.checks[0].evidence_positions = [...evidence];
    diagnostic.selected_excerpts = evidence.length;
    Object.assign(diagnostic.question, { verified_excerpts: evidence.length, requirements_supported: false,
      checks_by_kind: { value: { stated: 0, unavailable: 0, unsupported: 0 }, availability: { stated: 0, unavailable: 0, unsupported: 1 }, fact: { stated: 0, unavailable: 0, unsupported: 0 } } });
    raw.evaluation.checks[0].passed = false; raw.evaluation.quality_pass = false;
    assert.equal(verifyGuardedDiagnostic(raw, manifest, target).semantic_diagnostic.question.requirements_supported, false);
  }
});
test("guarded v2 rejects malformed trace shapes and never exposes raw diagnostic fields", async () => {
  for (const mutate of [
    raw => delete raw.semantic_diagnostic.trace,
    raw => raw.semantic_diagnostic.schema = "ollama-answer-guarded-diagnostic-v1",
    raw => raw.semantic_diagnostic.trace.raw_output = "secret",
    raw => raw.semantic_diagnostic.trace.catalog_size = 0,
    raw => raw.semantic_diagnostic.trace.catalog_size = 65,
    raw => raw.semantic_diagnostic.trace.catalog_size = 1.5,
    raw => raw.semantic_diagnostic.trace.catalog_size = "secret",
    raw => raw.semantic_diagnostic.trace.selected_positions = {},
    raw => raw.semantic_diagnostic.trace.selected_positions = [0],
    raw => raw.semantic_diagnostic.trace.selected_positions = [4],
    raw => raw.semantic_diagnostic.trace.selected_positions = [1, 1],
    raw => raw.semantic_diagnostic.trace.selected_positions = [2, 1],
    raw => raw.semantic_diagnostic.trace.selected_positions = [1.5],
    raw => raw.semantic_diagnostic.trace.selected_positions = ["secret"],
    raw => raw.semantic_diagnostic.trace.planned_kinds = {},
    raw => raw.semantic_diagnostic.trace.planned_kinds = [],
    raw => raw.semantic_diagnostic.trace.planned_kinds = Array(13).fill("availability"),
    raw => raw.semantic_diagnostic.trace.planned_kinds = ["secret"],
    raw => raw.semantic_diagnostic.trace.checks = {},
    raw => raw.semantic_diagnostic.trace.checks = [],
    raw => raw.semantic_diagnostic.trace.checks = Array(13).fill(raw.semantic_diagnostic.trace.checks[0]),
    raw => raw.semantic_diagnostic.trace.checks[0].text = "secret",
    raw => raw.semantic_diagnostic.trace.checks[0].kind = "secret",
    raw => raw.semantic_diagnostic.trace.checks[0].support = "secret",
    raw => raw.semantic_diagnostic.trace.checks[0].evidence_positions = {},
    raw => raw.semantic_diagnostic.trace.checks[0].evidence_positions = [],
    raw => raw.semantic_diagnostic.trace.checks[0].evidence_positions = [0],
    raw => raw.semantic_diagnostic.trace.checks[0].evidence_positions = [4],
    raw => raw.semantic_diagnostic.trace.checks[0].evidence_positions = [1, 1],
    raw => raw.semantic_diagnostic.trace.checks[0].evidence_positions = [2, 1],
    raw => raw.semantic_diagnostic.trace.checks[0].evidence_positions = [1.5],
    raw => raw.semantic_diagnostic.trace.checks[0].evidence_positions = ["secret"],
  ]) {
    const raw = traceFixture(); mutate(raw);
    const result = await runCases([manifest], target, async () => raw, verifyGuardedDiagnostic);
    assert.equal(result.complete, false); assert.deepEqual(result.results, []);
    assert.ok(!JSON.stringify(result).includes("secret"));
  }
});
test("guarded v2 rejects bounded trace contradictions that aggregate v1 checks cannot detect", async () => {
  for (const mutate of [
    raw => raw.semantic_diagnostic.trace.selected_positions = [2],
    raw => raw.semantic_diagnostic.trace.selected_positions = [1, 2],
    raw => raw.semantic_diagnostic.trace.checks[0].evidence_positions = [2],
    raw => raw.semantic_diagnostic.trace.checks[0].evidence_positions = [1, 2],
    raw => raw.semantic_diagnostic.trace.checks[0].kind = "availability",
    raw => raw.semantic_diagnostic.trace.checks[0].support = "stated",
    raw => raw.semantic_diagnostic.trace.planned_kinds = ["value"],
    raw => raw.semantic_diagnostic.verifier_kind_disagreements = 0,
    raw => raw.semantic_diagnostic.question.verified_excerpts = 2,
  ]) {
    const raw = traceFixture(); mutate(raw);
    const result = await runCases([manifest], target, async () => raw, verifyGuardedDiagnostic);
    assert.equal(result.complete, false); assert.deepEqual(result.results, []);
  }
});
