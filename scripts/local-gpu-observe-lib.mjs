import { performance } from "node:perf_hooks";
import { setTimeout as delay } from "node:timers/promises";
import { reserveMiB, headroomMiB } from "./local-model-budget.mjs";
export const scenarios = ["unspecified", "model", "game", "game-model"];
export function parseArgs(args) {
  const values = new Map();
  for (let i = 0; i < args.length; i += 2) {
    if (!["--seconds", "--scenario"].includes(args[i]) || values.has(args[i]) || typeof args[i + 1] !== "string") throw new Error("invalid observation arguments");
    values.set(args[i], args[i + 1]);
  }
  const seconds = values.get("--seconds") ?? "30";
  const scenario = values.get("--scenario") ?? "unspecified";
  if (!/^[1-9][0-9]{0,3}$/.test(seconds) || Number(seconds) > 3600 || !scenarios.includes(scenario)) throw new Error("invalid observation duration or scenario");
  return { durationMs: Number(seconds) * 1000, scenario };
}
function validGpu(gpu) {
  return gpu && /^GPU-[a-zA-Z0-9-]+$/.test(gpu.uuid) && typeof gpu.name === "string" && gpu.name.length > 0 && gpu.name.length <= 128
    && Number.isSafeInteger(gpu.totalMiB) && gpu.totalMiB > 0 && Number.isSafeInteger(gpu.freeMiB) && gpu.freeMiB >= 0 && gpu.freeMiB <= gpu.totalMiB;
}
// Read-only sampling. Never starts/stops a model, game or any other process.
export async function observeGpu({ durationMs, scenario, query, now = () => performance.now(), wait = delay, stopping = () => false }) {
  if (!Number.isSafeInteger(durationMs) || durationMs < 1000 || durationMs > 3600000 || !scenarios.includes(scenario)) throw new Error("invalid observation configuration");
  const start = now();
  let lastSample = start, gap = 0, samples = 0, attempts = 0, gpu, initialFree, lastFree, minFree, peakUsed;
  let belowReserve = 0, belowSupervisor = 0, failure;
  while (now() - start < durationMs && !stopping()) {
    attempts++;
    let reading;
    try { reading = await query(); }
    catch { failure = "gpu_query_unavailable"; break; }
    if (!validGpu(reading)) { failure = "gpu_reading_invalid"; break; }
    if (gpu && (reading.uuid !== gpu.uuid || reading.totalMiB !== gpu.totalMiB)) { failure = "gpu_identity_changed"; break; }
    if (!gpu) { gpu = { uuid: reading.uuid, name: reading.name, totalMiB: reading.totalMiB }; initialFree = reading.freeMiB; }
    const timestamp = now();
    gap = Math.max(gap, timestamp - lastSample); lastSample = timestamp; samples++;
    lastFree = reading.freeMiB;
    minFree = Math.min(minFree ?? reading.freeMiB, reading.freeMiB);
    peakUsed = Math.max(peakUsed ?? 0, reading.totalMiB - reading.freeMiB);
    if (reading.freeMiB < reserveMiB) belowReserve++;
    if (reading.freeMiB < reserveMiB + headroomMiB) belowSupervisor++;
    const remaining = durationMs - (now() - start);
    if (remaining > 0 && !stopping()) await wait(Math.min(500, remaining));
  }
  const elapsed = now() - start;
  gap = Math.max(gap, now() - lastSample);
  const interrupted = stopping();
  return { schema: "gpu-observation-v1", scenario, scenario_source: "user_label", game_activity_verified: false,
    model_activity_verified: false, continuous_peak_verified: false,
    status: failure ? "failed" : interrupted ? "interrupted" : "completed", ...(failure ? { failure } : {}),
    requested_ms: durationMs, elapsed_ms: Math.round(elapsed), sample_interval_ms: 500, max_sample_gap_ms: Math.round(gap),
    query_attempts: attempts, samples, gpu: gpu ?? null,
    initial_free_mib: initialFree ?? null, last_free_mib: lastFree ?? null, min_sampled_free_mib: minFree ?? null,
    peak_sampled_used_mib: peakUsed ?? null, reserve_mib: reserveMiB, supervisor_threshold_mib: reserveMiB + headroomMiB,
    below_reserve_samples: belowReserve, below_supervisor_samples: belowSupervisor,
    exit_code: failure ? 1 : interrupted ? 130 : belowSupervisor > 0 ? 2 : 0 };
}
