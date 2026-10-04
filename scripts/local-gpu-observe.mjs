import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { readGpu } from "./local-model-budget.mjs";
import { parseArgs, observeGpu } from "./local-gpu-observe-lib.mjs";
import { newReportDirectory, writeReport } from "./local-report.mjs";
const root = fileURLToPath(new URL("../", import.meta.url));
async function main() {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--help") {
    console.log("用法：node scripts/local-gpu-observe.mjs [--seconds 1..3600] [--scenario unspecified|model|game|game-model]\n默认 30 秒/unspecified，每约 500 ms 只读采样，报告保存私有目录。场景是用户标签，不验证游戏正在运行；采样峰值不保证捕获瞬时峰值。退出 0 观测完成，2 低于 6656 MiB 保护阈值，1 查询失败，130 中断；不操作模型或游戏进程。"); return;
  }
  const options = parseArgs(args);
  const directory = await newReportDirectory(join(root, ".local-model"), "observations");
  const digest = bytes => createHash("sha256").update(bytes).digest("hex");
  const provenance = {
    observer_sha256: digest(await readFile(fileURLToPath(import.meta.url))),
    sampling_sha256: digest(await readFile(join(root, "scripts/local-gpu-observe-lib.mjs"))),
    budget_sha256: digest(await readFile(join(root, "scripts/local-model-budget.mjs"))),
    runtime_config_sha256: digest(await readFile(join(root, "infra/local-model-runtime.json"))),
  };
  let stopped = false;
  const halt = () => { stopped = true; }; process.on("SIGINT", halt); process.on("SIGTERM", halt);
  try {
    const started = new Date().toISOString();
    const observation = await observeGpu({ ...options, query: readGpu, stopping: () => stopped });
    const report = { ...observation, started_at: started, ended_at: new Date().toISOString(), provenance };
    const path = await writeReport(directory, report);
    console.log(JSON.stringify({ report: path, ...observation }));
    process.exitCode = observation.exit_code;
  } finally { process.removeListener("SIGINT", halt); process.removeListener("SIGTERM", halt); }
}
try { await main(); } catch { console.error("GPU 观测参数、查询或报告准备未确认；未操作模型或游戏进程。"); process.exitCode = 1; }
