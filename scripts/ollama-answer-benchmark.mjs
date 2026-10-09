import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { open, unlink, lstat, readFile, realpath } from "node:fs/promises";
import { randomUUID } from "node:crypto";
import { fileURLToPath } from "node:url";
import { join, resolve, sep } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import assert from "node:assert/strict";
import { newReportDirectory, writeReport } from "./local-report.mjs";
import { runCases, PROTOCOL_STAGES } from "./local-answer-benchmark-lib.mjs";
import { availableMemory, digest, guardedSend, localMetadata as api, memoryBudget, memoryReady, parseOptions, qualityGate, runtimeIdentity, REPORT_SCHEMA, RESOURCE_POLICY, RUNNER, SUITES } from "./ollama-answer-benchmark-lib.mjs";

const execute = promisify(execFile);
const root = fileURLToPath(new URL("../", import.meta.url));
const base = join(root, ".local-model");
const binary = join(root, "target/debug/local-answer-benchmark");
// Child probes never inherit database, browser, API or provider credentials.
const env = { PATH: process.env.PATH, HOME: process.env.HOME, ...(process.env.RUSTUP_TOOLCHAIN ? { RUSTUP_TOOLCHAIN: process.env.RUSTUP_TOOLCHAIN } : {}) };
let stage = "arguments";
async function identity(target, signal) {
  const version = await api(target.endpoint, "/api/version", null, signal);
  const tags = await api(target.endpoint, "/api/tags", null, signal);
  const show = await api(target.endpoint, "/api/show", { model: target.model_alias, runner: RUNNER }, signal);
  return runtimeIdentity(target, version, tags, show);
}
async function idle(target, signal, afterOwnRequest = false) {
  const bounded = afterOwnRequest ? AbortSignal.any([signal, AbortSignal.timeout(2000)]) : signal;
  for (;;) {
    const state = await api(target.endpoint, "/api/ps", null, bounded);
    assert.ok(Array.isArray(state.models));
    if (state.models.length === 0) return;
    assert.ok(afterOwnRequest && state.models.every(model => model.digest === target.model_digest), "Ollama is busy");
    await delay(100, undefined, { signal: bounded });
  }
}
async function available(total) {
  const value = await execute("/usr/bin/memory_pressure", ["-Q"], { env, timeout: 3000, maxBuffer: 16384 });
  return availableMemory(total, value.stdout);
}
async function cli(args, signal) {
  const value = await execute(binary, args, { cwd: root, env, signal, timeout: 90000, maxBuffer: 65536 });
  return JSON.parse(value.stdout);
}
async function manifests(target, suite, signal) {
  return cli(["manifest", target.endpoint, target.model_alias, "--suite", suite, "--backend", "ollama"], signal);
}
async function gate(files, signal) {
  assert.equal(files.length, Object.keys(SUITES).length * 2);
  const seen = new Set(), reports = [];
  const allowed = await realpath(join(base, "quality"));
  for (const file of files) {
    const path = resolve(file), info = await lstat(path), actual = await realpath(path);
    assert.ok(info.isFile() && !info.isSymbolicLink() && info.size <= 65536 && actual.startsWith(`${allowed}${sep}`) && !seen.has(actual));
    seen.add(actual); reports.push(JSON.parse(await readFile(path, "utf8")));
  }
  const target = reports[0].runtime;
  parseOptions(["--endpoint", target.endpoint, "--model", target.model_alias]);
  const expected = {};
  for (const suite of Object.keys(SUITES)) expected[suite] = await manifests(target, suite, signal);
  const result = qualityGate(reports, expected);
  console.log(JSON.stringify(result)); process.exitCode = result.passed ? 0 : 2;
}
async function run() {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--help") {
    console.log("node scripts/ollama-answer-benchmark.mjs preview CASE [--suite baseline|challenge|coverage|extraction] [--model qwen3.5:9b] [--endpoint http://127.0.0.1:11434]\nnode scripts/ollama-answer-benchmark.mjs run [相同选项]\nnode scripts/ollama-answer-benchmark.mjs diagnose CASE [相同选项，单题合成诊断，不能计入门槛]\nnode scripts/ollama-answer-benchmark.mjs gate BASELINE1 BASELINE2 CHALLENGE1 CHALLENGE2 COVERAGE1 COVERAGE2 EXTRACTION1 EXTRACTION2\npreview/gate 离线，不发起模型推理；run/diagnose 仅使用内置合成资料与已安装本地模型，要求 macOS ARM64、空闲 Ollama、至少 6 GiB+512 MiB 内存估计余量。8192 上下文、2048 输出，think=false、keep_alive=0，无下载/自动重试。退出 0 全通过、2 质量失败、1 未确认。"); return;
  }
  const command = args[0];
  assert.ok(command === "run" || command === "gate" || ["preview", "diagnose"].includes(command) && /^[a-z0-9_]{1,40}$/.test(args[1] ?? ""));
  const target = command === "gate" ? null : parseOptions(args.slice(command === "run" ? 1 : 2));
  const abort = new AbortController(); const halt = () => abort.abort();
  process.on("SIGINT", halt); process.on("SIGTERM", halt);
  let lock, lockPath;
  try {
    stage = "build";
    await execute("cargo", ["build", "--locked", "--offline", "-p", "api-server", "--bin", "local-answer-benchmark"], { cwd: root, env, timeout: 180000, signal: abort.signal, maxBuffer: 65536 });
    if (command === "gate") { stage = "gate"; await gate(args.slice(1), abort.signal); return; }
    if (command === "preview") {
      stage = "offline_preview";
      console.log(JSON.stringify(await cli(["preview", args[1], target.endpoint, target.model_alias, "--suite", target.suite, "--backend", "ollama"], abort.signal))); return;
    }
    assert.ok(process.platform === "darwin" && process.arch === "arm64", "run requires macOS ARM64 memory guard");
    stage = "report_directory";
    const directory = await newReportDirectory(base, "quality");
    stage = "project_lock";
    lockPath = join(base, "benchmark.lock"); lock = await open(lockPath, "wx", 0o600);
    await lock.writeFile(JSON.stringify({ pid: process.pid }));
    stage = "system_memory";
    const total = Number((await execute("/usr/sbin/sysctl", ["-n", "hw.memsize"], { env, timeout: 3000, maxBuffer: 16384 })).stdout.trim());
    assert.ok(Number.isSafeInteger(total) && total > 0);
    stage = "local_runtime";
    const runtime = { ...await identity({ endpoint: target.endpoint, model_alias: target.model_alias }, abort.signal),
      platform: process.platform, arch: process.arch, total_memory: total, resource_policy: RESOURCE_POLICY };
    stage = "idle_runtime";
    await idle(runtime, abort.signal);
    stage = "frozen_manifest";
    const all = await manifests(target, target.suite, abort.signal);
    const frozen = command === "diagnose" ? all.filter(item => item.case.id === args[1]) : all;
    assert.ok(frozen.length > 0);
    const started = new Date().toISOString();
    const resources = { preload_checks: 0, inflight_checks: 0, minimum_preload_available_bytes: null, minimum_inflight_available_bytes: null, settle_wait_ms: 0 };
    stage = "synthetic_cases";
    const outcome = await runCases(frozen, runtime, async id => {
      try {
        await idle(runtime, abort.signal);
        const current = await identity({ endpoint: target.endpoint, model_alias: target.model_alias }, abort.signal);
        assert.equal(digest(current), digest(currentIdentity(runtime)));
      } catch { throw Object.assign(new Error("local runtime changed or busy"), { answerFailure: "runtime" }); }
      try {
        const ready = await memoryReady(() => available(total), runtime.model_size, abort.signal);
        resources.preload_checks++;
        resources.minimum_preload_available_bytes = Math.min(resources.minimum_preload_available_bytes ?? total, ready.available);
        resources.settle_wait_ms += ready.waited_ms;
      }
      catch { throw Object.assign(new Error("local memory reserve unconfirmed"), { answerFailure: "resource" }); }
      try {
        const raw = await guardedSend(signal => cli(["case", id, target.endpoint, target.model_alias, "--use-local-benchmark", "--suite", target.suite, "--backend", "ollama"], signal),
          async () => {
            const free = await available(total); resources.inflight_checks++;
            resources.minimum_inflight_available_bytes = Math.min(resources.minimum_inflight_available_bytes ?? total, free);
            memoryBudget(free, runtime.model_size, false);
          }, abort.signal);
        try {
          await idle(runtime, abort.signal, true);
          assert.equal(digest(await identity({ endpoint: target.endpoint, model_alias: target.model_alias }, abort.signal)), digest(currentIdentity(runtime)));
        } catch { throw Object.assign(new Error("local runtime changed or still busy"), { answerFailure: "runtime" }); }
        console.error(`Ollama ${target.suite}: ${id} completed`);
        return raw;
      } catch (error) {
        const fixed = /^ANSWER_FAILURE=(transport|protocol)$/m.exec(error.stderr ?? "");
        const protocolStage = /^ANSWER_PROTOCOL_STAGE=([a-z_]+)$/m.exec(error.stderr ?? "")?.[1];
        throw Object.assign(new Error("local answer unconfirmed"), { answerFailure: error.answerFailure ?? fixed?.[1], answerFailureStage: PROTOCOL_STAGES.includes(protocolStage) ? protocolStage : undefined });
      }
    });
    const report = { schema: REPORT_SCHEMA, run_id: randomUUID(), synthetic_only: true, suite: target.suite,
      ...(command === "diagnose" ? { diagnostic_only: true } : {}),
      started_at: started, ended_at: new Date().toISOString(), runtime, resources, manifests: frozen, ...outcome,
      passed_cases: outcome.results.filter(result => result.quality_pass).length, total_cases: frozen.length };
    stage = "write_report";
    console.log(JSON.stringify({ report: await writeReport(directory, report), complete: report.complete,
      passed_cases: report.passed_cases, total_cases: report.total_cases, failure: report.failure, failure_stage: report.failure_stage, diagnostic_only: report.diagnostic_only, exit_code: report.exit_code }));
    process.exitCode = report.exit_code;
  } finally {
    process.removeListener("SIGINT", halt); process.removeListener("SIGTERM", halt);
    if (lock) { await lock.close(); await unlink(lockPath); }
  }
}
function currentIdentity(runtime) {
  return Object.fromEntries(Object.entries(runtime).filter(([key]) => !["platform", "arch", "total_memory", "resource_policy"].includes(key)));
}
try { await run(); }
catch { console.error(`Ollama 问答评估未确认（${stage}）；检查已安装本地模型、资源余量、锁文件与报告。不会下载模型、重试推理或停止共享服务。`); process.exitCode = 1; }
