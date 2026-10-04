import { test } from "node:test";
import assert from "node:assert/strict";
import { parseGpu, canLoad, canContinue, canSend, watchBudget, reserveMiB, headroomMiB, loadBudgetMiB } from "./local-model-budget.mjs";
test("GPU query refuses unavailable, partial, multi-device or impossible readings", () => {
  for (const text of ["", "GPU-x,RTX,N/A,9000", "GPU-x,RTX,100,101", "GPU-x,RTX,16000,-1", "GPU-x,RTX,16000,9000\nGPU-y,RTX,16000,9000"]) assert.throws(() => parseGpu(text));
});
test("sending distinguishes a resident model from a sleeping model without double counting weights", () => {
  const gpu = { freeMiB: 8500 };
  assert.equal(canSend(gpu, false), true);
  assert.equal(canSend(gpu, true), false);
  assert.equal(canSend({ freeMiB: 7500 }, false), false);
});
test("monitor stops exactly once on lost reserve or unknown GPU and never restarts", async () => {
  for (const fault of ["low", "unavailable", "identity"]) {
    let queries = 0, stopped = 0;
    await watchBudget({ gpuUuid: "GPU-x", stopping: () => false, wait: async () => {},
      query: async () => { queries++; if (queries === 1) return { uuid: "GPU-x", freeMiB: 12000 }; if (fault === "unavailable") throw new Error("query unavailable"); return { uuid: fault === "identity" ? "GPU-y" : "GPU-x", freeMiB: fault === "low" ? 6143 : 12000 }; },
      violate: () => { stopped++; },
    });
    assert.equal(queries, 2); assert.equal(stopped, 1);
  }
});
test("admission includes game reserve, monitor headroom and model loading peak", () => {
  const gpu = parseGpu("GPU-abcd, NVIDIA GeForce RTX 5070 Ti, 16303, 14000\n");
  const required = reserveMiB + headroomMiB + loadBudgetMiB;
  assert.equal(canLoad({ ...gpu, freeMiB: required - 1 }), false);
  assert.equal(canLoad({ ...gpu, freeMiB: required }), true);
  assert.equal(canContinue({ ...gpu, freeMiB: reserveMiB + headroomMiB - 1 }), false);
  assert.equal(canContinue({ ...gpu, freeMiB: reserveMiB + headroomMiB }), true);
});
