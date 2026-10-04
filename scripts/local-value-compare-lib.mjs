import assert from "node:assert/strict";
import { verifyCase } from "./local-value-benchmark-lib.mjs";
const hash = v => typeof v === "string" && /^[0-9a-f]{64}$/.test(v);
const key = v => typeof v === "string" && /^[a-z0-9_]{1,40}$/.test(v);
const text = v => typeof v === "string" && /^[a-zA-Z0-9_.:/\[\]-]{1,128}$/.test(v);
const failures = ["transport", "output_json", "output_schema", "output_count", "output_ids", "output_score", "output_reason", "output_reason_category", "output_classification", "output_strict", "case_execution_or_protocol_unconfirmed"];
function keys(value, expected) { assert.deepEqual(Object.keys(value).sort(), expected.sort()); }
function validateManifest(m) {
  keys(m, ["id", "corpus_sha256", "input_digest", "request_sha256", "execution_profile", "quality_version", "prompt_bytes", "checks", "items"]);
  assert.ok(key(m.id) && hash(m.corpus_sha256) && hash(m.input_digest) && hash(m.request_sha256));
  assert.ok(["local-rss-v1", "local-rss-v2", "local-rss-v3", "local-rss-v4"].includes(m.execution_profile));
  assert.ok(["rss-quality-v1", "rss-challenge-v1", "rss-regression-v1"].includes(m.quality_version));
  assert.ok(Number.isSafeInteger(m.prompt_bytes) && m.prompt_bytes > 0 && m.prompt_bytes <= 5632);
  assert.ok(Array.isArray(m.items) && m.items.length > 0 && m.items.length <= 8 && m.items.every(key));
  assert.equal(new Set(m.items).size, m.items.length);
  assert.ok(Array.isArray(m.checks) && m.checks.length > 0 && m.checks.length <= 16);
  for (const c of m.checks) {
    if (["minimum", "ceiling"].includes(c.kind)) {
      keys(c, ["kind", "item", "value"]); assert.ok(m.items.includes(c.item));
      assert.ok(Number.isInteger(c.value) && c.value >= 0 && c.value <= 100);
    } else if (c.kind === "abstain") {
      keys(c, ["kind", "item"]); assert.ok(m.items.includes(c.item));
    } else if (c.kind === "prefer") {
      keys(c, ["kind", "higher", "lower", "margin"]); assert.ok(m.items.includes(c.higher) && m.items.includes(c.lower) && c.higher !== c.lower);
      assert.ok(Number.isInteger(c.margin) && c.margin > 0 && c.margin <= 100);
    } else {
      assert.equal(c.kind, "no_canary"); keys(c, ["kind", "text"]);
      assert.ok(typeof c.text === "string" && /^[\x20-\x7e]{1,80}$/.test(c.text));
    }
  }
}
export function validateReport(r) {
  assert.equal(r.schema, "rss-quality-report-v1"); assert.equal(r.synthetic_only, true);
  assert.ok(r.run_scope === undefined || r.run_scope === "full_corpus");
  const fields = ["schema", "synthetic_only", "started_at", "ended_at", "runtime", "manifests", "complete", "results", "exit_code", "passed_cases", "total_cases"];
  if (r.run_scope !== undefined) fields.push("run_scope");
  if (r.complete === false) fields.push("failure", "failed_case");
  keys(r, fields);
  assert.ok(typeof r.started_at === "string" && r.started_at.length <= 32 && Number.isFinite(Date.parse(r.started_at)));
  assert.ok(typeof r.ended_at === "string" && r.ended_at.length <= 32 && Date.parse(r.ended_at) >= Date.parse(r.started_at));
  keys(r.runtime, ["engine", "configured_version", "config_sha256", "server_sha256", "launcher_sha256", "model_sha256", "model_revision", "endpoint", "model_alias"]);
  for (const field of ["config_sha256", "server_sha256", "launcher_sha256", "model_sha256"]) assert.ok(hash(r.runtime[field]));
  for (const field of ["engine", "configured_version", "model_revision", "endpoint", "model_alias"]) assert.ok(text(r.runtime[field]));
  assert.ok(Array.isArray(r.manifests) && r.manifests.length > 0 && r.manifests.length <= 8);
  r.manifests.forEach(validateManifest);
  assert.equal(new Set(r.manifests.map(m => m.id)).size, r.manifests.length);
  for (const m of r.manifests) {
    assert.equal(m.corpus_sha256, r.manifests[0].corpus_sha256);
    assert.equal(m.execution_profile, r.manifests[0].execution_profile);
    assert.equal(m.quality_version, r.manifests[0].quality_version);
  }
  assert.equal(r.total_cases, r.manifests.length);
  assert.ok(Array.isArray(r.results) && r.results.length <= r.manifests.length);
  r.results.forEach((result, index) => {
    const m = r.manifests[index];
    const projected = verifyCase({ elapsed_ms: result.elapsed_ms, response_bytes: result.response_bytes,
      endpoint: r.runtime.endpoint, model: r.runtime.model_alias, synthetic_only: true,
      result: { manifest: m, protocol_valid: result.protocol_valid, quality_pass: result.quality_pass, checks: result.checks, scores: result.scores } }, m, r.runtime);
    assert.deepEqual(result, projected);
    // Verify score-derived checks; redacted reasons cannot independently prove no_canary.
    m.checks.forEach((c, i) => {
      let expected;
      if (c.kind === "minimum") expected = result.scores[c.item] !== null && result.scores[c.item] >= c.value;
      else if (c.kind === "ceiling") expected = result.scores[c.item] === null || result.scores[c.item] <= c.value;
      else if (c.kind === "abstain") expected = result.scores[c.item] === null;
      else if (c.kind === "prefer") expected = result.scores[c.higher] !== null && (result.scores[c.lower] === null || result.scores[c.higher] - result.scores[c.lower] >= c.margin);
      if (expected !== undefined) assert.equal(result.checks[i].passed, expected);
    });
  });
  assert.equal(r.passed_cases, r.results.filter(c => c.quality_pass).length);
  if (r.complete === true) {
    assert.equal(r.results.length, r.manifests.length);
    assert.equal(r.exit_code, r.passed_cases === r.total_cases ? 0 : 2);
  } else {
    assert.equal(r.complete, false); assert.equal(r.exit_code, 1);
    assert.ok(r.results.length < r.manifests.length);
    assert.equal(r.failed_case, r.manifests[r.results.length].id); assert.ok(failures.includes(r.failure));
  }
  return r;
}
export function compareReports(inputs) {
  assert.ok(Array.isArray(inputs) && inputs.length >= 2 && inputs.length <= 10);
  const reports = inputs.map(validateReport), first = reports[0];
  for (const r of reports.slice(1)) {
    assert.deepEqual(r.manifests, first.manifests, "different corpus, checks, request or profile");
    assert.deepEqual(r.runtime, first.runtime, "different runtime or model fingerprints");
  }
  const cases = first.manifests.map((m, index) => {
    const completed = reports.flatMap(r => r.results[index] ? [r.results[index]] : []);
    const failed = reports.filter(r => r.failed_case === m.id);
    const scoreSignatures = new Set(completed.map(r => JSON.stringify(m.items.map(item => r.scores[item]))));
    const checkSignatures = new Set(completed.map(r => JSON.stringify(r.checks.map(c => c.passed))));
    return { id: m.id, completed_runs: completed.length, protocol_failed_runs: failed.length,
      not_run: reports.length - completed.length - failed.length,
      quality_passed_runs: completed.filter(r => r.quality_pass).length,
      quality_failed_runs: completed.filter(r => !r.quality_pass).length,
      repeated_identically: completed.length === reports.length && scoreSignatures.size === 1 && checkSignatures.size === 1,
      elapsed_ms: completed.length ? { min: Math.min(...completed.map(r => r.elapsed_ms)), max: Math.max(...completed.map(r => r.elapsed_ms)) } : null,
      checks: m.checks.map((c, i) => ({ index: i, kind: c.kind, passed_runs: completed.filter(r => r.checks[i].passed).length, failed_runs: completed.filter(r => !r.checks[i].passed).length })),
      scores: Object.fromEntries(m.items.map(item => {
        const values = completed.map(r => r.scores[item]), numeric = values.filter(v => v !== null);
        return [item, { numeric_runs: numeric.length, abstained_runs: values.filter(v => v === null).length,
          min: numeric.length ? Math.min(...numeric) : null, max: numeric.length ? Math.max(...numeric) : null }];
      })),
      failures: Object.fromEntries([...new Set(failed.map(r => r.failure))].map(f => [f, failed.filter(r => r.failure === f).length])),
    };
  });
  return { schema: "rss-quality-comparison-v1", runs: reports.length, comparable: true,
    corpus_sha256: first.manifests[0].corpus_sha256, execution_profile: first.manifests[0].execution_profile,
    model_sha256: first.runtime.model_sha256, all_passed: reports.every(r => r.exit_code === 0),
    repeated_identically: cases.every(c => c.repeated_identically), complete_runs: reports.filter(r => r.complete).length, cases };
}
