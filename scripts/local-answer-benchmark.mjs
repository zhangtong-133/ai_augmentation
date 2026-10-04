import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createReadStream } from "node:fs";
import { open, unlink } from "node:fs/promises";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { newReportDirectory, writeReport } from "./local-report.mjs";
import { runCases } from "./local-answer-benchmark-lib.mjs";
import { modelChoice, modelCommandOptions } from "./local-model-choice.mjs";
const execute = promisify(execFile);
const root = fileURLToPath(new URL("../", import.meta.url));
const base = join(root, ".local-model");
const binary = join(root, "target/debug/local-answer-benchmark");
const env = { PATH: process.env.PATH, HOME: process.env.HOME, ...(process.env.RUSTUP_TOOLCHAIN ? { RUSTUP_TOOLCHAIN: process.env.RUSTUP_TOOLCHAIN } : {}) };
async function hashFile(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}
async function run() {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--help") {
    console.log("make local-answer-benchmark-preview CASE=single_fact | make local-answer-benchmark\n可用 MODEL=qwen3-8b 显式选择候选；仅内置合成材料，无数据库/API 凭据。先启动受至少 6 GiB 显存保护的项目模型，每场景发送一次，无自动重试。退出 0 全通过、2 质量失败、1 未确认；只保存检查元数据。"); return;
  }
  if (args[0] !== "run" && !(args[0] === "preview" && /^[a-z0-9_]{1,40}$/.test(args[1] ?? ""))) throw new Error("invalid answer benchmark command");
  const model = modelCommandOptions(args.slice(args[0] === "run" ? 1 : 2));
  const abort = new AbortController(); const halt = () => abort.abort();
  process.on("SIGINT", halt); process.on("SIGTERM", halt);
  let lock, lockPath;
  try {
    const directory = args[0] === "run" ? await newReportDirectory(base, "quality") : null;
    if (directory) {
      lockPath = join(base, "benchmark.lock"); lock = await open(lockPath, "wx", 0o600);
      await lock.writeFile(JSON.stringify({ pid: process.pid }));
    }
    await execute("cargo", ["build", "--locked", "--offline", "-p", "api-server", "--bin", "local-answer-benchmark"], { cwd: root, env, timeout: 180000, signal: abort.signal, maxBuffer: 65536 });
    const { config, configSha256 } = await modelChoice(model);
    const target = [config.endpoint, config.model_alias];
    if (args[0] === "preview") {
      const result = await execute(binary, ["preview", args[1], ...target], { cwd: root, env, timeout: 10000, signal: abort.signal, maxBuffer: 65536 });
      process.stdout.write(result.stdout); return;
    }
    const modelSha = await hashFile(join(base, config.model.filename));
    if (modelSha !== config.model.sha256) throw new Error("model differs from pinned file");
    const runtime = { engine: config.engine, configured_version: config.version, config_sha256: configSha256,
      server_sha256: await hashFile(join(base, `llama-runtime/llama-${config.version}/llama-server`)),
      launcher_sha256: await hashFile(join(base, "llama-server")), model_sha256: modelSha,
      model_revision: config.model.revision, endpoint: config.endpoint, model_alias: config.model_alias };
    const raw = await execute(binary, ["manifest", ...target], { cwd: root, env, timeout: 10000, signal: abort.signal, maxBuffer: 65536 });
    const manifests = JSON.parse(raw.stdout);
    const started = new Date().toISOString();
    const outcome = await runCases(manifests, config, async id => {
      try {
        const result = await execute(process.execPath, [join(root, "scripts/local-model.mjs"), "answer-benchmark-case", id, "--model", model], { cwd: root, env, timeout: 90000, signal: abort.signal, maxBuffer: 65536 });
        return JSON.parse(result.stdout);
      } catch (error) {
        const fixed = /^ANSWER_FAILURE=(transport|protocol)$/m.exec(error.stderr ?? "");
        throw Object.assign(new Error("answer benchmark unconfirmed"), { answerFailure: fixed?.[1] });
      }
    });
    const report = { schema: "knowledge-answer-quality-report-v1", synthetic_only: true, started_at: started, ended_at: new Date().toISOString(), runtime, manifests, ...outcome,
      passed_cases: outcome.results.filter(r => r.quality_pass).length, total_cases: manifests.length };
    console.log(JSON.stringify({ report: await writeReport(directory, report), complete: report.complete, passed_cases: report.passed_cases, total_cases: report.total_cases, exit_code: report.exit_code }));
    process.exitCode = report.exit_code;
  } finally {
    process.removeListener("SIGINT", halt); process.removeListener("SIGTERM", halt);
    if (lock) { await lock.close(); await unlink(lockPath); }
  }
}
try { await run(); } catch { console.error("问答基准准备或执行未确认；检查模型、报告及锁文件，不自动重试。"); process.exitCode = 1; }
