import { test } from "node:test";
import assert from "node:assert/strict";
import { runCases, verifyCase } from "./local-answer-benchmark-lib.mjs";
import { verifyEvidenceRoleDiagnostic } from "./ollama-answer-evidence-role-diagnostics.mjs";
import { verifyGuardedDiagnostic } from "./ollama-answer-guarded-diagnostics.mjs";

const manifest = { execution_profile: "ollama-knowledge-answer-v20-evidence-role", case: { id: "fixture", expected_status: "answered" }, request_sha256: "a".repeat(64) };
const target = { endpoint: "http://127.0.0.1:11434", model_alias: "fixture" };
const counts = () => ({ stated: 0, unavailable: 0, unsupported: 0 });
function fixture(kind = "value", basis = "asserted_fact", support = "stated", evidence = [1]) {
  const supported = support === "stated" || kind === "availability" && support === "unavailable";
  const planned = { value: 0, availability: 0, fact: 0 }, checks = { value: counts(), availability: counts(), fact: counts() };
  planned[kind] = 1; checks[kind][support] = 1;
  return { manifest, endpoint: target.endpoint, model: target.model_alias, synthetic_only: true, protocol_valid: true, elapsed_ms: 1,
    evaluation: { citation_valid: true, quality_pass: supported, checks: ["expected_status", "required_terms", "expected_citations", "forbidden_terms"].map(name => ({ name, passed: name === "forbidden_terms" || supported })) },
    semantic_diagnostic: { schema: "ollama-answer-evidence-role-diagnostic-v1", basis: [basis], guarded: {
      schema: "ollama-answer-guarded-diagnostic-v2", selected_excerpts: evidence.length, evidence_agrees: true, verifier_kind_disagreements: 0,
      question: { schema: "ollama-answer-question-diagnostic-v1", planned_requirements: 1, verified_requirements: 1,
        requirements_agree: true, requirements_supported: supported, verified_excerpts: evidence.length, planned_by_kind: planned, checks_by_kind: checks },
      trace: { catalog_size: 3, selected_positions: [...evidence], planned_kinds: [kind], checks: [{ kind, support, evidence_positions: [...evidence] }] },
    } } };
}

test("evidence roles preserve factual answers and restrict explicit absence to availability questions", () => {
  for (const [kind, basis, support, answered] of [
    ["value", "asserted_fact", "stated", true],
    ["availability", "explicit_absence", "unavailable", true],
    ["value", "explicit_absence", "unavailable", false],
    ["fact", "instruction", "unsupported", false],
    ["availability", "unrelated", "unsupported", false],
    ["availability", "missing", "unsupported", false],
  ]) {
    const raw = fixture(kind, basis, support, basis === "missing" ? [] : [1]);
    const result = verifyEvidenceRoleDiagnostic(raw, manifest, target);
    assert.equal(result.quality_pass, answered);
    assert.deepEqual(result.semantic_diagnostic, raw.semantic_diagnostic);
    assert.notEqual(result.semantic_diagnostic.basis, raw.semantic_diagnostic.basis);
    assert.notEqual(result.semantic_diagnostic.guarded.trace, raw.semantic_diagnostic.guarded.trace);
    assert.throws(() => verifyCase(raw, manifest, target));
    assert.throws(() => verifyGuardedDiagnostic(raw, manifest, target));
  }
  for (const role of ["instruction", "unrelated", "missing"]) for (const evidence of [[], [1]]) {
    const result = verifyEvidenceRoleDiagnostic(fixture("availability", role, "unsupported", evidence), manifest, target);
    assert.equal(result.semantic_diagnostic.guarded.question.requirements_supported, false);
    assert.deepEqual(result.semantic_diagnostic.guarded.trace.checks[0].evidence_positions, evidence);
  }
});

test("instruction evidence cannot become support through independent selection agreement or availability kind", () => {
  const raw = fixture("availability", "instruction", "unsupported");
  assert.equal(raw.semantic_diagnostic.guarded.evidence_agrees, true);
  assert.equal(verifyEvidenceRoleDiagnostic(raw, manifest, target).checks[0].passed, false);
  const disguised = fixture("availability", "instruction", "stated");
  assert.throws(() => verifyEvidenceRoleDiagnostic(disguised, manifest, target));
  const missingAsAbsent = fixture("availability", "missing", "unavailable");
  assert.throws(() => verifyEvidenceRoleDiagnostic(missingAsAbsent, manifest, target));
});

test("requirement count mismatches retain all roles and still validate every support mapping", () => {
  const raw = fixture();
  const value = raw.semantic_diagnostic, guarded = value.guarded;
  value.basis.push("instruction");
  guarded.trace.checks.push({ kind: "availability", support: "unsupported", evidence_positions: [] });
  Object.assign(guarded.question, { verified_requirements: 2, requirements_agree: false, requirements_supported: false, checks_by_kind: null });
  guarded.verifier_kind_disagreements = null;
  raw.evaluation.checks[0].passed = false; raw.evaluation.quality_pass = false;
  assert.equal(verifyEvidenceRoleDiagnostic(raw, manifest, target).semantic_diagnostic.basis.length, 2);
  value.basis[1] = "asserted_fact";
  assert.throws(() => verifyEvidenceRoleDiagnostic(raw, manifest, target));
});

test("malformed roles and nested traces stop reporting without exposing model text or sending later cases", async () => {
  for (const mutate of [
    raw => delete raw.semantic_diagnostic,
    raw => raw.semantic_diagnostic.raw_output = "secret",
    raw => raw.semantic_diagnostic.schema = "secret",
    raw => raw.semantic_diagnostic.basis = "asserted_fact",
    raw => raw.semantic_diagnostic.basis = [],
    raw => raw.semantic_diagnostic.basis = Array(13).fill("asserted_fact"),
    raw => raw.semantic_diagnostic.basis = ["asserted_fact", "instruction"],
    raw => raw.semantic_diagnostic.basis = ["secret"],
    raw => raw.semantic_diagnostic.basis = ["__proto__"],
    raw => raw.semantic_diagnostic.basis = ["toString"],
    raw => raw.semantic_diagnostic.basis = [["asserted_fact"]],
    raw => raw.semantic_diagnostic.basis = [{ role: "asserted_fact", text: "secret" }],
    raw => raw.semantic_diagnostic.basis = [null],
    raw => raw.semantic_diagnostic.basis = [1],
    raw => raw.semantic_diagnostic.basis = ["instruction"],
    raw => raw.semantic_diagnostic.guarded.schema = "ollama-answer-guarded-diagnostic-v1",
    raw => raw.semantic_diagnostic.guarded.question.answer = "secret",
    raw => raw.semantic_diagnostic.guarded.trace.checks[0].evidence_positions = [],
    raw => raw.semantic_diagnostic.guarded.trace.checks[0].evidence_positions = [2],
    raw => raw.semantic_diagnostic.guarded.trace.selected_positions = [2],
    raw => raw.semantic_diagnostic.guarded.question.requirements_supported = false,
  ]) {
    const raw = fixture(); mutate(raw); let calls = 0;
    const result = await runCases([manifest, { ...manifest, case: { ...manifest.case, id: "later" } }], target, async () => { calls++; return raw; }, verifyEvidenceRoleDiagnostic);
    assert.equal(calls, 1); assert.equal(result.complete, false);
    assert.deepEqual(result.results, []); assert.deepEqual(result.not_run, ["later"]);
    assert.ok(!JSON.stringify(result).includes("secret"));
  }
});
