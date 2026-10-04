import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createReadStream } from "node:fs";
import { open, readFile, unlink } from "node:fs/promises";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { newReportDirectory, writeReport } from "./local-report.mjs";
import { runCases } from "./local-value-benchmark-lib.mjs";
import { benchmarkOptions } from "./local-benchmark-options.mjs";
const execute = promisify(execFile);
const root = fileURLToPath(new URL("../", import.meta.url));
const base = join(root, ".local-model");
const binary = join(root, "target/debug/local-value-benchmark");
const env = { PATH: process.env.PATH, HOME: process.env.HOME, ...(process.env.RUSTUP_TOOLCHAIN ? { RUSTUP_TOOLCHAIN: process.env.RUSTUP_TOOLCHAIN } : {}) };
async function hashFile(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}
async function run() {
  const { args, flags } = benchmarkOptions(process.argv.slice(2));
  if (args.length === 1 && args[0] === "--help") {
    console.log("用法：make local-value-benchmark-preview CASE=rust_preference | make local-value-benchmark\n先显式启动本项目模型；SUITE=baseline 四组或 SUITE=challenge 六组固定合成语料各发送一次。退出 0 全通过，2 质量失败，1 执行/协议未确认；报告存项目私有目录，不自动重试。"); return;
  }
  if (!(args.length === 1 && args[0] === "run") && !(args.length === 2 && args[0] === "preview" && /^[a-z0-9_]{1,40}$/.test(args[1]))) throw new Error("invalid benchmark command");
  const abort = new AbortController();
  const halt = () => abort.abort(); process.on("SIGINT", halt); process.on("SIGTERM", halt);
  let lock, lockPath;
  try {
    if (args[0] === "run") {
      // mkdir/check permissions first, then exclusive whole-run lock, no stale-lock takeover.
      const directory = await newReportDirectory(base, "quality");
      lockPath = join(base, "benchmark.lock");
      lock = await open(lockPath, "wx", 0o600);
      await lock.writeFile(JSON.stringify({ pid: process.pid }));
      await execute("cargo", ["build", "--locked", "--offline", "-p", "api-server", "--bin", "local-value-benchmark"], { cwd: root, env, timeout: 180000, signal: abort.signal, maxBuffer: 65536 });
      const configBytes = await readFile(join(root, "infra/local-model-runtime.json"));
      const config = JSON.parse(configBytes);
      if (config.endpoint !== "http://127.0.0.1:11435" || config.model_alias !== "qwen3:4b-q4_K_M" || !/^b[0-9]+$/.test(config.version)) throw new Error("configured target differs from protected launcher");
      const modelSha = await hashFile(join(base, config.model.filename));
      if (modelSha !== config.model.sha256) throw new Error("model fingerprint differs from pinned model");
      const runtime = { engine: config.engine, configured_version: config.version,
        config_sha256: createHash("sha256").update(configBytes).digest("hex"),
        server_sha256: await hashFile(join(base, `llama-runtime/llama-${config.version}/llama-server`)),
        launcher_sha256: await hashFile(join(base, "llama-server")),
        model_sha256: modelSha, model_revision: config.model.revision,
        endpoint: config.endpoint, model_alias: config.model_alias };
      const { stdout } = await execute(binary, ["manifest", ...flags], { cwd: root, env, timeout: 10000, signal: abort.signal, maxBuffer: 65536 });
      const manifests = JSON.parse(stdout);
      const started = new Date().toISOString();
      const outcome = await runCases(manifests, config, async id => {
        try {
          const result = await execute(process.execPath, [join(root, "scripts/local-model.mjs"), "benchmark-case", id, ...flags],
            { cwd: root, env, timeout: 90000, signal: abort.signal, maxBuffer: 65536 });
          return JSON.parse(result.stdout);
        } catch (error) {
          const fixed = /^BENCH_FAILURE=(transport|output_json|output_schema|output_count|output_ids|output_score|output_reason|output_reason_category|output_strict)$/m.exec(error.stderr ?? "");
          throw Object.assign(new Error("benchmark unconfirmed"), { benchmarkFailure: fixed?.[1] });
        }
      });
      const report = { schema: "rss-quality-report-v1", run_scope: "full_corpus", synthetic_only: true, started_at: started, ended_at: new Date().toISOString(),
        runtime, manifests, ...outcome, passed_cases: outcome.results.filter(r => r.quality_pass).length, total_cases: manifests.length };
      const path = await writeReport(directory, report);
      console.log(JSON.stringify({ report: path, complete: report.complete, passed_cases: report.passed_cases,
        total_cases: report.total_cases, exit_code: report.exit_code }));
      process.exitCode = report.exit_code;
    } else {
      await execute("cargo", ["build", "--locked", "--offline", "-p", "api-server", "--bin", "local-value-benchmark"], { cwd: root, env, timeout: 180000, signal: abort.signal, maxBuffer: 65536 });
      const { stdout } = await execute(binary, ["preview", args[1], ...flags], { cwd: root, env, timeout: 10000, signal: abort.signal, maxBuffer: 65536 });
      process.stdout.write(stdout);
    }
  } finally {
    process.removeListener("SIGINT", halt); process.removeListener("SIGTERM", halt);
    if (lock) { await lock.close(); await unlink(lockPath); }
  }
}
try { await run(); } catch { console.error("基准准备或执行未确认；核对模型、私有报告及锁文件，不自动重试。"); process.exitCode = 1; }
