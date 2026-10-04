import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { readReport } from "./local-quality-report.mjs";
import { validateReport } from "./local-value-compare-lib.mjs";
import { publicGate } from "./local-value-public-lib.mjs";
import { newReportDirectory, writeReport } from "./local-report.mjs";
const execute = promisify(execFile);
async function main() {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--help") {
    console.log("用法：node scripts/local-value-public.mjs 报告.json ...（1–10份）\n离线核对当前 v4 的公开文档改写校准集与留出集，各至少两轮、相同运行配置、全部条件通过且重复一致。报告包含逐场景给分、弃权、上限错误和 partial 偏差。退出 0 固定材料通过，2 条件未通过，1 无法核实；不代表人工复核、真实用户质量或业务启用。"); return;
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
  for (const suite of ["public_calibration", "public_holdout"]) {
    const { stdout } = await execute(join(root, "target/debug/local-value-benchmark"), ["manifest", "--suite", suite], { cwd: root, env, timeout: 10000, maxBuffer: 65536 });
    expected[suite] = JSON.parse(stdout);
  }
  const gate = { ...publicGate(inputs.map(i => i.report), expected), source_sha256: inputs.map(i => i.sha256) };
  const path = await writeReport(await newReportDirectory(join(root, ".local-model"), "quality"), gate);
  console.log(JSON.stringify({ report: path, public_fixture_gate_passed: gate.public_fixture_gate_passed,
    suites: gate.suites.map(({ comparison, ...rest }) => rest) }));
  process.exitCode = gate.public_fixture_gate_passed ? 0 : 2;
}
try { await main(); } catch { console.error("公开材料验收无法核实：报告无效、重复、重叠或不匹配当前冻结条件；未调用模型、未修改业务配置。"); process.exitCode = 1; }
