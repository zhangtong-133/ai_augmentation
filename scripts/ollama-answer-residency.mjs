import assert from "node:assert/strict";
import { memoryReady, RUNNER } from "./ollama-answer-benchmark-lib.mjs";

// ps observes residency, not exclusive ownership or absence of concurrent external requests.
export function residentState(runtime, state, now = Date.now()) {
  assert.ok(Array.isArray(state.models) && state.models.length <= 1, "Ollama residency unconfirmed");
  if (state.models.length === 0) return { state: "cold", loading: true };
  const model = state.models[0];
  assert.equal(model.name, runtime.model_alias);
  assert.equal(model.model, runtime.model_alias);
  assert.equal(model.digest, runtime.model_digest);
  assert.equal(model.details?.format, "gguf");
  // Older ps responses omit runner; they can never establish warm reuse.
  for (const runner of [model.runner, model.details.runner]) assert.ok(runner === undefined || runner === RUNNER);
  assert.ok(!model.remote_model && !model.remote_host);
  assert.ok(Number.isSafeInteger(model.size) && model.size > 0);
  assert.ok(Number.isSafeInteger(model.size_vram) && model.size_vram >= 0);
  assert.ok(Number.isSafeInteger(model.context_length) && model.context_length > 0);
  const expiry = Date.parse(model.expires_at);
  assert.ok(Number.isFinite(expiry));
  // A different context or imminent expiry may cause a reload; require its full loading budget.
  const reusable = (model.runner ?? model.details.runner) === RUNNER && model.context_length === 8192 && expiry - now >= 10000;
  return { state: reusable ? "reuse" : "reload", loading: !reusable };
}

export async function confirmResidentRequest(runtime, query, sample, signal, settleMs = 15000, intervalMs = 500) {
  let before;
  try { before = residentState(runtime, await query(signal)); }
  catch { throw Object.assign(new Error("local residency unconfirmed"), { answerFailure: "runtime" }); }
  let ready;
  try { ready = await memoryReady(sample, runtime.model_size, signal, settleMs, intervalMs, before.loading); }
  catch { throw Object.assign(new Error("local memory reserve unconfirmed"), { answerFailure: "resource" }); }
  try {
    const after = residentState(runtime, await query(signal));
    assert.deepEqual(after, before);
    if (signal?.aborted) throw new Error("cancelled before native permission");
  } catch { throw Object.assign(new Error("local residency changed before native permission"), { answerFailure: "runtime" }); }
  return { ...before, ...ready };
}
