import { readFileSync } from "node:fs";
import { SUITES, RESERVE_BYTES, REUSE_HEADROOM_BYTES, executionPolicy } from "./ollama-answer-benchmark-lib.mjs";

const frozen = name => JSON.parse(readFileSync(new URL(`../apps/api-server/tests/fixtures/${name}`, import.meta.url), "utf8"));
const original = frozen("answer-independent-manifests-ollama-v14.json");
const profiles = {
  independent: frozen("answer-residency-manifests-ollama-v15.json"),
  question: frozen("answer-question-manifests-ollama-v16.json"),
  guarded: frozen("answer-guarded-manifests-ollama-v17.json"),
  "question-first": frozen("answer-question-first-manifests-ollama-v18.json"),
  "evidence-bound": frozen("answer-evidence-bound-manifests-ollama-v19.json"),
};
export function answerReports({ residency = "reuse", candidate = "independent", offset = 0 } = {}) {
  const policy = executionPolicy(residency, candidate), requests = ["guarded", "question-first", "evidence-bound"].includes(candidate) ? 3 : 2;
  const expected = Object.fromEntries(Object.keys(SUITES).map(suite => [suite, original[suite].map(manifest => ({
    case: structuredClone(manifest.case), execution_profile: policy.profile,
    request_sha256: residency === "cold" ? manifest.request_sha256 : profiles[candidate].suites[suite][manifest.case.id],
  }))]));
  const reports = Object.keys(SUITES).flatMap(suite => [suite, suite]).map((suite, index) => {
    const total = expected[suite].length, permits = total * requests;
    return { schema: policy.report_schema, run_id: `00000000-0000-4000-8000-${String(offset + index).padStart(12, "0")}`,
      synthetic_only: true, suite,
      started_at: new Date((offset + index + 1) * 100000).toISOString(), ended_at: new Date((offset + index + 1) * 100000 + 1000).toISOString(),
      runtime: { engine: "ollama", runner: "llamacpp", version: "0.40.3", execution_profile: policy.profile,
        endpoint: "http://127.0.0.1:11434", model_alias: "qwen3.5:9b", model_digest: "a".repeat(64), model_metadata_sha256: "b".repeat(64),
        model_size: 1000, platform: "darwin", arch: "arm64", total_memory: 32 * 1024 ** 3, resource_policy: policy.resource_policy },
      resources: { preload_checks: permits, inflight_checks: total, minimum_preload_available_bytes: RESERVE_BYTES + 1500,
        minimum_inflight_available_bytes: RESERVE_BYTES, settle_wait_ms: 0,
        ...(residency === "reuse" ? { cold_checks: 1, reload_checks: 0, reuse_checks: permits - 1,
          minimum_loading_available_bytes: RESERVE_BYTES + 1500, minimum_reuse_available_bytes: RESERVE_BYTES + REUSE_HEADROOM_BYTES } : {}) },
      manifests: structuredClone(expected[suite]), total_cases: total, passed_cases: total, complete: true, not_run: [], exit_code: 0,
      results: expected[suite].map(manifest => ({ id: manifest.case.id, request_sha256: manifest.request_sha256,
        elapsed_ms: 100, protocol_valid: true, citation_valid: true, quality_pass: true,
        checks: ["expected_status", "required_terms", "expected_citations", "forbidden_terms"].map(name => ({ name, passed: true })) })),
    };
  });
  return { reports, expected };
}

export function failCase(reports, suite, id, names) {
  for (const report of reports.filter(report => report.suite === suite)) {
    const result = report.results.find(result => result.id === id);
    result.checks.forEach(check => { check.passed = !names.includes(check.name); });
    result.quality_pass = result.checks.every(check => check.passed);
    report.passed_cases = report.results.filter(result => result.quality_pass).length;
    report.exit_code = report.passed_cases === report.total_cases ? 0 : 2;
  }
}
