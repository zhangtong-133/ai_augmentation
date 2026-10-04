// Synthetic reports used by offline validator tests only.
export function report(time = "2026-10-04T12:00:00.000Z") {
  const manifests = ["a", "b", "c"].map(id => ({ id, corpus_sha256: "a".repeat(64), input_digest: "b".repeat(64), request_sha256: "c".repeat(64),
    execution_profile: "local-rss-v2", quality_version: "rss-quality-v1", prompt_bytes: 100, items: ["article"], checks: [{ kind: "minimum", item: "article", value: 60 }] }));
  return { schema: "rss-quality-report-v1", run_scope: "full_corpus", synthetic_only: true, started_at: time, ended_at: time,
    runtime: { engine: "llama.cpp", configured_version: "b123", config_sha256: "d".repeat(64), server_sha256: "e".repeat(64), launcher_sha256: "f".repeat(64), model_sha256: "0".repeat(64), model_revision: "1".repeat(40), endpoint: "http://127.0.0.1:11435", model_alias: "qwen3:4b" },
    manifests, complete: true, exit_code: 0, total_cases: 3, passed_cases: 3,
    results: manifests.map(m => ({ id: m.id, input_digest: m.input_digest, request_sha256: m.request_sha256,
      protocol_valid: true, quality_pass: true, elapsed_ms: 400, response_bytes: 80, scores: { article: 80 }, checks: [{ index: 0, passed: true }] })) };
}
