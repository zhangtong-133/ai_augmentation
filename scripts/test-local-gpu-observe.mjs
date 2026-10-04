import { test } from "node:test";
import assert from "node:assert/strict";
import { parseArgs, observeGpu } from "./local-gpu-observe-lib.mjs";
const gpu = { uuid: "GPU-test", name: "Synthetic GPU", totalMiB: 16303, freeMiB: 12000 };
async function sample(readings, { durationMs = 1500, interruptAfter, queryTime = 0, scenario = "game-model" } = {}) {
  let clock = 0, queries = 0;
  const observation = await observeGpu({ durationMs, scenario,
    now: () => clock, wait: async ms => { clock += ms; }, stopping: () => interruptAfter !== undefined && queries >= interruptAfter,
    query: async () => { clock += queryTime; const reading = readings[Math.min(queries++, readings.length - 1)]; if (reading instanceof Error) throw reading; return { ...gpu, ...reading }; },
  });
  return { observation, queries };
}
test("duration and scenario labels are explicit and bounded, with duplicate or malformed flags refused", () => {
  assert.deepEqual(parseArgs([]), { durationMs: 30000, scenario: "unspecified" });
  assert.deepEqual(parseArgs(["--seconds", "3600", "--scenario", "game"]), { durationMs: 3600000, scenario: "game" });
  for (const args of [["--seconds", "0"], ["--seconds", "3601"], ["--seconds", "1.5"], ["--seconds", "01"], ["--seconds"], ["--scenario", "automatic"], ["--seconds", "1", "--seconds", "2"], ["--start-model", "true"]]) assert.throws(() => parseArgs(args));
});
test("sampled peak and 6 GiB reserve distinguish the stricter supervisor buffer without claiming game verification", async () => {
  const { observation: r } = await sample([{ freeMiB: 6656 }, { freeMiB: 6144 }, { freeMiB: 6143 }]);
  assert.equal(r.status, "completed"); assert.equal(r.exit_code, 2); assert.equal(r.samples, 3);
  assert.equal(r.min_sampled_free_mib, 6143); assert.equal(r.peak_sampled_used_mib, 16303 - 6143);
  assert.equal(r.below_reserve_samples, 1); assert.equal(r.below_supervisor_samples, 2);
  assert.equal(r.max_sample_gap_ms, 500); assert.equal(r.game_activity_verified, false); assert.equal(r.scenario_source, "user_label");
  assert.equal((await sample([{ freeMiB: 6656 }])).observation.exit_code, 0);
});
test("unknown query, invalid memory and changed device or capacity stop with bounded failure metadata", async () => {
  for (const fault of [new Error("private process info"), { freeMiB: NaN }, { uuid: "GPU-other" }, { totalMiB: 15000 }]) {
    const { observation: r, queries } = await sample([gpu, fault, gpu]);
    assert.equal(queries, 2); assert.equal(r.exit_code, 1); assert.equal(r.status, "failed");
    assert.equal(r.samples, 1); assert.equal(r.gpu.uuid, "GPU-test");
    assert.ok(!JSON.stringify(r).includes("private process info"));
  }
});
test("slow queries and tail gaps remain visible; interrupted capture reports partial observations", async () => {
  const slow = (await sample([gpu], { durationMs: 1000, queryTime: 1400 })).observation;
  assert.equal(slow.elapsed_ms, 1400); assert.equal(slow.max_sample_gap_ms, 1400); assert.equal(slow.samples, 1);
  const stopped = (await sample([gpu], { interruptAfter: 1 })).observation;
  assert.equal(stopped.status, "interrupted"); assert.equal(stopped.exit_code, 130); assert.equal(stopped.samples, 1);
  assert.equal(stopped.elapsed_ms, 0);
});
