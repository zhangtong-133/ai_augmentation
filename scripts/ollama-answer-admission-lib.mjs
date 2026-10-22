import assert from "node:assert/strict";
import { availableMemory, digest, localTarget, memoryBudget, REUSE_HEADROOM_BYTES, runtimeIdentity } from "./ollama-answer-benchmark-lib.mjs";
import { residentState } from "./ollama-answer-residency.mjs";

// Shared runtime/resource primitives only. No benchmark manifest, report or quality gate is read.
export function admissionArguments(args) {
  assert.equal(args.length, 3, "invalid admission arguments");
  const [command, endpoint, model] = args;
  assert.ok(["answer-admit", "answer-monitor", "inspect"].includes(command), "invalid admission command");
  const target = localTarget(endpoint, model);
  return { command, target };
}

export async function identity(target, query, signal) {
  const [version, tags, show] = await Promise.all([
    query(target.endpoint, "/api/version", null, signal),
    query(target.endpoint, "/api/tags", null, signal),
    query(target.endpoint, "/api/show", { model: target.model_alias, runner: "llamacpp" }, signal),
  ]);
  const { execution_profile: ignored, ...runtime } = runtimeIdentity(target, version, tags, show);
  void ignored;
  return runtime;
}

function cancelled(signal) {
  if (signal?.aborted) throw new Error("local admission cancelled");
}

async function snapshot(target, query, signal) {
  const [runtime, state] = await Promise.all([
    identity(target, query, signal),
    query(target.endpoint, "/api/ps", null, signal),
  ]);
  return { runtime, residency: residentState(runtime, state) };
}

function reserve(total, pressure, runtime, residency, monitor) {
  const available = availableMemory(total, pressure);
  // Once generation is in flight its allocation is already reflected in available memory.
  // Requiring another load or idle headroom budget would double count that allocation.
  memoryBudget(available, runtime.model_size, !monitor && residency.loading,
    monitor || residency.loading ? 0 : REUSE_HEADROOM_BYTES);
  return available;
}

// This is a bounded, read-only observation. /api/ps does not prove exclusive server ownership.
// Process/database permits belong to the API. Existing benchmark locks are never altered.
export async function admit(target, { query, totalMemory, memoryPressure, benchmarkActive, platform, arch, signal }, command = "answer-admit") {
  assert.ok(["answer-admit", "answer-monitor", "inspect"].includes(command), "invalid admission command");
  const monitor = command === "answer-monitor";
  assert.equal(platform, "darwin", "requires macOS memory observation");
  assert.equal(arch, "arm64", "requires Apple unified memory observation");
  localTarget(target.endpoint, target.model_alias);
  cancelled(signal);
  assert.equal(await benchmarkActive(), false, "project benchmark is active");
  const before = await snapshot(target, query, signal);
  const fingerprint = digest(before.runtime);
  const total = Number(await totalMemory(signal));
  assert.ok(Number.isSafeInteger(total) && total > 0);
  const availableBefore = reserve(total, await memoryPressure(signal), before.runtime, before.residency, monitor);
  cancelled(signal);
  const after = await snapshot(target, query, signal);
  if (monitor) assert.deepEqual(after.runtime, before.runtime, "local runtime changed");
  else assert.deepEqual(after, before, "local runtime or residency changed");
  const availableAfter = reserve(total, await memoryPressure(signal), after.runtime, after.residency, monitor);
  assert.equal(await benchmarkActive(), false, "project benchmark is active");
  cancelled(signal);
  return { identity_sha256: fingerprint, ...before.runtime, residency: after.residency.state,
    total_memory: total, minimum_available_bytes: Math.min(availableBefore, availableAfter) };
}
