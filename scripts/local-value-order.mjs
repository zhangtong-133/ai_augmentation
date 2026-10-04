import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { readReport } from "./local-quality-report.mjs";
import { validateReport } from "./local-value-compare-lib.mjs";
import { auditOrder } from "./local-value-order-lib.mjs";
import { newReportDirectory, writeReport } from "./local-report.mjs";
const execute = promisify(execFile);
async function main() {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--help") {
    console.log("用法：node scripts/local-value-order.mjs 报告1.json 报告2.json [最多10份]\n离线验收当前冻结 order 套件；相同 profile/模型/运行配置、独立不重叠的至少两轮，各轮质量通过、换序一致且跨轮重复一致才通过。失败/未运行不视为弃权。退出 0 通过，2 条件未通过，1 无法核实；不调用模型或更改业务配置。"); return;
  }
  if (args.length < 2 || args.length > 10 || args.some(a => a.startsWith("-"))) throw new Error("invalid arguments");
  const inputs = [];
  for (const path of args) inputs.push(await readReport(path));
  if (new Set(inputs.map(i => i.sha256)).size !== inputs.length) throw new Error("duplicate reports");
  inputs.forEach(i => validateReport(i.report));
  const profile = inputs[0].report.manifests[0].execution_profile;
  const root = fileURLToPath(new URL("../", import.meta.url));
  const env = { PATH: process.env.PATH, HOME: process.env.HOME, ...(process.env.RUSTUP_TOOLCHAIN ? { RUSTUP_TOOLCHAIN: process.env.RUSTUP_TOOLCHAIN } : {}) };
  await execute("cargo", ["build", "--locked", "--offline", "-p", "api-server", "--bin", "local-value-benchmark"], { cwd: root, env, timeout: 180000, maxBuffer: 65536 });
  const { stdout } = await execute(join(root, "target/debug/local-value-benchmark"), ["manifest", "--suite", "order", "--profile", profile], { cwd: root, env, timeout: 10000, maxBuffer: 65536 });
  const audit = { ...auditOrder(inputs.map(i => i.report), JSON.parse(stdout)), source_sha256: inputs.map(i => i.sha256) };
  const path = await writeReport(await newReportDirectory(join(root, ".local-model"), "quality"), audit);
  console.log(JSON.stringify({ report: path, order_gate_passed: audit.order_gate_passed,
    execution_profile: profile, coverage: audit.comparison.coverage, rounds: audit.rounds }));
  process.exitCode = audit.order_gate_passed ? 0 : 2;
}
try { await main(); } catch { console.error("换序验收无法核实：报告无效、重复、重叠或不匹配当前冻结条件；未调用模型、未修改业务配置。"); process.exitCode = 1; }
