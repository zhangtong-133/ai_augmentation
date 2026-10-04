import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { access } from "node:fs/promises";
const execute = promisify(execFile);
export const reserveMiB = 6 * 1024;
export const headroomMiB = 512;
export const loadBudgetMiB = 4 * 1024;
export const generationHeadroomMiB = 1024;
export function parseGpu(csv) {
  const rows = csv.trim().split("\n");
  if (rows.length !== 1) throw new Error("需明确选择单个 GPU");
  const parts = rows[0].split(",").map(v => v.trim());
  if (parts.length !== 4 || !/^GPU-[a-zA-Z0-9-]+$/.test(parts[0])) throw new Error("无法核实 GPU 显存");
  const total = Number(parts[2]), free = Number(parts[3]);
  if (!Number.isSafeInteger(total) || !Number.isSafeInteger(free) || free < 0 || total <= 0 || free > total) throw new Error("无法核实 GPU 显存");
  return { uuid: parts[0], name: parts[1], totalMiB: total, freeMiB: free };
}
export function canLoad(gpu) { return gpu.freeMiB >= reserveMiB + headroomMiB + loadBudgetMiB; }
export function canContinue(gpu) { return gpu.freeMiB >= reserveMiB + headroomMiB; }
export function canSend(gpu, sleeping) {
  return sleeping ? canLoad(gpu) : gpu.freeMiB >= reserveMiB + headroomMiB + generationHeadroomMiB;
}
export async function watchBudget({ query, gpuUuid, stopping, wait, violate }) {
  while (!stopping()) {
    await wait();
    if (stopping()) return;
    try {
      const gpu = await query();
      if (gpu.uuid !== gpuUuid || !canContinue(gpu)) throw new Error("显存余量不足");
    } catch { violate(); return; }
  }
}
export async function readGpu() {
  let binary = "nvidia-smi";
  try { await access("/usr/lib/wsl/lib/nvidia-smi"); binary = "/usr/lib/wsl/lib/nvidia-smi"; } catch { /* native Linux */ }
  const { stdout } = await execute(binary, ["--id=0", "--query-gpu=uuid,name,memory.total,memory.free", "--format=csv,noheader,nounits"], { timeout: 2000, maxBuffer: 4096 });
  return parseGpu(stdout);
}
