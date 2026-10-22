import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
const require = createRequire(new URL("../apps/web/package.json", import.meta.url));
const ts = require("typescript");
const source = ts.transpileModule(await readFile(new URL("../apps/web/src/components/answer-authorization-types.ts", import.meta.url), "utf8"), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 },
}).outputText;
const { isGrant } = await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);
function fixture() {
  return {
    request_id: "00000000-0000-4000-8000-000000000123", status: "consumed", digest: "a".repeat(64),
    created_at_unix_ms: 1, expires_at_unix_ms: 600001, execution_available: true, preview: null,
    execution: { status: "succeeded", request_sha256: "b".repeat(64), started_at_unix_ms: 2, deadline_at_unix_ms: 55002,
      sent_at_unix_ms: 3, finished_at_unix_ms: 4, question: "何时开会？", result: {
        status: "answered", answer: "会议安排在周五。[1]", citations: [{
          id: 1, document_id: "source", title: "会议", source: "fixture.md", ordinal: 0, score: 1,
          text: "😀会议安排在周五。结束", quote: "会议安排在周五。", quote_start: 1, quote_end: 9,
        }],
      } },
  };
}
test("execution result accepts Unicode scalar offsets and list metadata removes bodies", () => {
  const grant = fixture();
  assert.equal(isGrant(grant), true);
  assert.equal(isGrant(grant, true), false);
  const summary = structuredClone(grant); summary.execution.question = null; summary.execution.result = null;
  assert.equal(isGrant(summary, true), true);
  summary.execution.question = "private";
  assert.equal(isGrant(summary, true), false);
});
test("citation offsets reject UTF-16 indices, ambiguity and substituted source text", () => {
  for (const mutate of [
    citation => { citation.quote_start = 2; citation.quote_end = 10; },
    citation => { citation.text += citation.quote; },
    citation => { citation.text = "😀会议安排在周六。结束"; },
    citation => { citation.quote_end = 100; },
    citation => { citation.quote = " "; },
    citation => { citation.score = Number.NaN; },
    citation => { citation.ordinal = -1; },
  ]) {
    const grant = fixture(); mutate(grant.execution.result.citations[0]);
    assert.equal(isGrant(grant), false);
  }
});
test("result validation rejects duplicate IDs and malformed insufficient evidence", () => {
  const duplicate = fixture(); duplicate.execution.result.citations.push(structuredClone(duplicate.execution.result.citations[0]));
  assert.equal(isGrant(duplicate), false);
  const insufficient = fixture(); insufficient.execution.result = { status: "insufficient_evidence", answer: null, citations: [] };
  assert.equal(isGrant(insufficient), true);
  insufficient.execution.result.answer = "模型猜测";
  assert.equal(isGrant(insufficient), false);
  const empty = fixture(); empty.execution.result.answer = " ";
  assert.equal(isGrant(empty), false);
});
test("non-success states cannot carry an answer and executions require a consumed grant", () => {
  for (const status of ["running", "failed", "unknown", "cancelled", "invalidated"]) {
    const grant = fixture(); grant.execution.status = status;
    assert.equal(isGrant(grant), false);
    grant.execution.question = null; grant.execution.result = null;
    assert.equal(isGrant(grant), true);
  }
  const grant = fixture(); grant.status = "authorized";
  assert.equal(isGrant(grant), false);
});
test("invalid timestamps, capabilities and execution protocol fail closed", () => {
  for (const mutate of [
    grant => { grant.execution_available = "true"; },
    grant => { delete grant.execution; },
    grant => { grant.execution.deadline_at_unix_ms = 1; },
    grant => { grant.execution.sent_at_unix_ms = -1; },
    grant => { grant.execution.status = "retrying"; },
    grant => { grant.execution.request_sha256 = "bad"; },
  ]) {
    const grant = fixture(); mutate(grant); assert.equal(isGrant(grant), false);
  }
});
