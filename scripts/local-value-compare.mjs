import { readReport } from "./local-quality-report.mjs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { compareReports } from "./local-value-compare-lib.mjs";
import { newReportDirectory, writeReport } from "./local-report.mjs";
async function main() {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--help") {
    console.log("用法：node scripts/local-value-compare.mjs 报告1.json 报告2.json [最多10份]\n仅离线比较同语料/条件/请求/profile/模型及运行文件指纹的完整轮报告；部分失败轮保留未知/未运行，不当成零分。独立单场景不能混入。退出 0 全轮质量通过，2 可比较但质量/协议未全部通过，1 无法比较。私有比较报告不会覆盖原文件或调用模型。"); return;
  }
  if (args.length < 2 || args.length > 10 || args.some(a => a.startsWith("-"))) throw new Error("invalid comparison arguments");
  const inputs = [];
  for (const path of args) inputs.push(await readReport(path));
  if (new Set(inputs.map(r => r.sha256)).size !== inputs.length) throw new Error("duplicate report is not another run");
  const comparison = { ...compareReports(inputs.map(i => i.report)), source_sha256: inputs.map(i => i.sha256) };
  const root = fileURLToPath(new URL("../", import.meta.url));
  const directory = await newReportDirectory(join(root, ".local-model"), "quality");
  const path = await writeReport(directory, comparison);
  console.log(JSON.stringify({ report: path, ...comparison }));
  process.exitCode = comparison.all_passed ? 0 : 2;
}
try { await main(); } catch { console.error("无法比较：报告无效、重复、超限，或语料/请求/profile/模型指纹不同；未调用模型、未覆盖原报告。"); process.exitCode = 1; }
