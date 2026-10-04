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
test("larger candidate budget applies at startup and sleep wakeup without relaxing the reserve", () => {
  assert.equal(canLoad({ freeMiB: 12800 - 1 }, 6144), false);
  assert.equal(canLoad({ freeMiB: 12800 }, 6144), true);
  assert.equal(canSend({ freeMiB: 11000 }, true, 4096), true);
  assert.equal(canSend({ freeMiB: 11000 }, true, 6144), false);
  assert.equal(canSend({ freeMiB: 7680 }, false, 6144), true);
  for (const invalid of [0, -1, 6143, NaN, "6144"]) {
    assert.equal(canLoad({ freeMiB: 16000 }, invalid), false);
    assert.equal(canSend({ freeMiB: 16000 }, false, invalid), false);
  }
});
test("model selection binds alias, runtime config and admission budget to the owning supervisor", async () => {
  const { modelChoice, modelCommandOptions, validateOwnerChoice } = await import("./local-model-choice.mjs");
  const baseline = await modelChoice(), candidate = await modelChoice("qwen3-8b");
  assert.equal(baseline.config.load_budget_mib, 4096);
  assert.equal(candidate.config.load_budget_mib, 6144);
  assert.notEqual(baseline.configSha256, candidate.configSha256);
  assert.equal(modelCommandOptions([]), "qwen3-4b");
  for (const args of [["--model", "unknown"], ["--model", "qwen3-8b", "--model", "qwen3-4b"], ["--model"]]) assert.throws(() => modelCommandOptions(args));
  const state = { modelKey: candidate.key, model: candidate.config.model_alias, endpoint: candidate.config.endpoint,
    configSha256: candidate.configSha256, loadBudgetMiB: 6144 };
  validateOwnerChoice(state, candidate);
  assert.throws(() => validateOwnerChoice(state, baseline));
  for (const change of [{ model: baseline.config.model_alias }, { loadBudgetMiB: 4096 }, { configSha256: "changed" }]) {
    assert.throws(() => validateOwnerChoice({ ...state, ...change }, candidate));
  }
});
