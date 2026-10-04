import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { readReport } from "./local-quality-report.mjs";
import { validateReport } from "./local-value-compare-lib.mjs";
import { qualityGate } from "./local-value-gate-lib.mjs";
import { newReportDirectory, writeReport } from "./local-report.mjs";
const execute = promisify(execFile);
async function main() {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--help") {
    console.log("用法：node scripts/local-value-gate.mjs 报告.json ...（1–10份）\n离线检查当前 v2 基线与挑战集各至少两轮，所有报告须同一运行配置且全部通过。不调用模型，不修改业务配置。退出 0 合成门槛通过，2 门槛未通过，1 报告/条件无效。通过不代表真实材料质量或游戏显存峰值验收。"); return;
  }
  if (args.length < 1 || args.length > 10 || args.some(a => a.startsWith("-"))) throw new Error("invalid arguments");
  const inputs = [];
  for (const path of args) inputs.push(await readReport(path));
  if (new Set(inputs.map(i => i.sha256)).size !== inputs.length) throw new Error("duplicate reports");
  inputs.forEach(i => validateReport(i.report));
  const root = fileURLToPath(new URL("../", import.meta.url));
  const env = { PATH: process.env.PATH, HOME: process.env.HOME, ...(process.env.RUSTUP_TOOLCHAIN ? { RUSTUP_TOOLCHAIN: process.env.RUSTUP_TOOLCHAIN } : {}) };
  await execute("cargo", ["build", "--locked", "--offline", "-p", "api-server", "--bin", "local-value-benchmark"], { cwd: root, env, timeout: 180000, maxBuffer: 65536 });
  const expected = {};
  for (const suite of ["baseline", "challenge"]) {
    const { stdout } = await execute(join(root, "target/debug/local-value-benchmark"), ["manifest", "--suite", suite, "--profile", "local-rss-v2"], { cwd: root, env, timeout: 10000, maxBuffer: 65536 });
    expected[suite] = JSON.parse(stdout);
  }
  const gate = { ...qualityGate(inputs.map(i => i.report), expected), source_sha256: inputs.map(i => i.sha256) };
  const path = await writeReport(await newReportDirectory(join(root, ".local-model"), "quality"), gate);
  console.log(JSON.stringify({ report: path, synthetic_gate_passed: gate.synthetic_gate_passed,
    suites: gate.suites.map(({ comparison, ...summary }) => summary) }));
  process.exitCode = gate.synthetic_gate_passed ? 0 : 2;
}
try { await main(); } catch { console.error("质量门槛无法核实：报告无效、重复、重叠或不匹配当前冻结条件；未调用模型、未修改业务配置。"); process.exitCode = 1; }
