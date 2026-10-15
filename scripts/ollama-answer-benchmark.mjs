import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { open, unlink } from "node:fs/promises";
import { randomUUID } from "node:crypto";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import assert from "node:assert/strict";
import { stagedCli } from "./ollama-answer-stages.mjs";
import { DIAGNOSTIC_REPORT_SCHEMA, verifyDiagnostic } from "./ollama-answer-diagnostics.mjs";
import { verifyGuardedDiagnostic } from "./ollama-answer-guarded-diagnostics.mjs";
import { verifyQuestionDiagnostic } from "./ollama-answer-question-diagnostics.mjs";
import { confirmResidentRequest, residentState } from "./ollama-answer-residency.mjs";
import { newReportDirectory, writeReport } from "./local-report.mjs";
import { compareQualityReports } from "./ollama-answer-compare-lib.mjs";
import { comparisonFiles, expectedManifests, readQualityReports } from "./ollama-answer-reports.mjs";
import { runCases, PROTOCOL_STAGES } from "./local-answer-benchmark-lib.mjs";
import { availableMemory, digest, executionPolicy, guardedSend, localMetadata as api, memoryBudget, memoryReady, parseOptions, qualityGate, runtimeIdentity, RUNNER } from "./ollama-answer-benchmark-lib.mjs";

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
  return cli(["manifest", target.endpoint, target.model_alias, "--suite", suite, "--backend", "ollama", "--residency", target.residency, "--candidate", target.candidate], signal);
}
async function gate(inputs, signal) {
  const reports = inputs.map(input => input.report);
  const expected = await expectedManifests(reports, (target, suite) => manifests(target, suite, signal));
  const result = qualityGate(reports, expected);
  console.log(JSON.stringify(result)); process.exitCode = result.passed ? 0 : 2;
}
async function compare(inputs, signal) {
  const middle = inputs.length / 2;
  const left = inputs.slice(0, middle).map(input => input.report), right = inputs.slice(middle).map(input => input.report);
  const load = (target, suite) => manifests(target, suite, signal);
  const result = compareQualityReports(left, right, await expectedManifests(left, load), await expectedManifests(right, load));
  const directory = await newReportDirectory(base, "quality");
  console.log(JSON.stringify({ report: await writeReport(directory, result), ...result }));
  process.exitCode = result.all_passed ? 0 : 2;
}
async function run() {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--help") {
    console.log("node scripts/ollama-answer-benchmark.mjs preview CASE [--suite baseline|challenge|coverage|extraction|decision|mixed|availability|support] [--model qwen3.5:9b] [--endpoint http://127.0.0.1:11434] [--residency reuse|cold] [--candidate independent|question|guarded|question-first|evidence-bound]\nnode scripts/ollama-answer-benchmark.mjs run [相同选项]\nnode scripts/ollama-answer-benchmark.mjs diagnose CASE [相同选项，单题合成阶段及语义计数，不能计入门槛]\nnode scripts/ollama-answer-benchmark.mjs gate BASELINE1 BASELINE2 CHALLENGE1 CHALLENGE2 COVERAGE1 COVERAGE2 EXTRACTION1 EXTRACTION2 DECISION1 DECISION2 MIXED1 MIXED2 AVAILABILITY1 AVAILABILITY2 SUPPORT1 SUPPORT2\nnode scripts/ollama-answer-benchmark.mjs compare --left 十六份报告路径 --right 十六份报告路径\ncompare 要求同一运行时/模型和原八套条件，按套内时间配对两轮，保留整题与检查项退步；生成 0700/0600 私有报告，不授予执行权限。退出 0 双方门槛均通过、2 可比较但至少一方失败、1 无法比较。\npreview/gate/compare 离线，不发起模型推理；run/diagnose 仅使用内置合成资料与已安装本地模型，要求 macOS ARM64、至少 6 GiB+512 MiB 内存估计余量。默认 reuse：同一运行器/模型/8192 上下文可驻留复用，keep_alive=30 秒；发送前另留 1 GiB，冷加载/重配置仍需 1.5 倍模型大小。cold 保留 v14 逐次冷请求（keep_alive=0）。candidate 默认 independent；显式 question 仅用于 reuse 合成实验，先只读问题规划类型，再独立核对资料，两步均在发送前冻结。question-first 仅调整 guarded 的完整用户 JSON 字段顺序，将问题放在资料之前；evidence-bound 另将核对 schema 的 stated/unavailable 绑定非空证据；三者均为三步。guarded 将问题分类与原独立选择、核对组合，每题三次静态请求（plan/select/verify）；其他候选两次。每次独立检查身份、驻留和资源，每次 2048 输出、think=false；检测其他驻留模型拒绝发送，无下载/自动重试/强制卸载。退出 0 全通过、2 质量失败、1 未确认。"); return;
  }
  const command = args[0];
  assert.ok(["run", "gate", "compare"].includes(command) || ["preview", "diagnose"].includes(command) && /^[a-z0-9_]{1,64}$/.test(args[1] ?? ""));
  const offline = ["gate", "compare"].includes(command);
  const target = offline ? null : parseOptions(args.slice(command === "run" ? 1 : 2));
  const policy = target ? executionPolicy(target.residency, target.candidate) : null;
  const abort = new AbortController(); const halt = () => abort.abort();
  process.on("SIGINT", halt); process.on("SIGTERM", halt);
  let lock, lockPath;
  try {
    let inputs;
    if (offline) {
      stage = "read_reports";
      const files = command === "compare" ? comparisonFiles(args.slice(1)) : null;
      inputs = await readQualityReports(files ? [...files.left, ...files.right] : args.slice(1), base);
    }
    stage = "build";
    await execute("cargo", ["build", "--locked", "--offline", "-p", "api-server", "--bin", "local-answer-benchmark"], { cwd: root, env, timeout: 180000, signal: abort.signal, maxBuffer: 65536 });
    if (command === "gate") { stage = "gate"; await gate(inputs, abort.signal); return; }
    if (command === "compare") { stage = "compare"; await compare(inputs, abort.signal); return; }
    if (command === "preview") {
      stage = "offline_preview";
      console.log(JSON.stringify(await cli(["preview", args[1], target.endpoint, target.model_alias, "--suite", target.suite, "--backend", "ollama", "--residency", target.residency, "--candidate", target.candidate], abort.signal))); return;
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
    const runtime = { ...await identity(target, abort.signal),
      platform: process.platform, arch: process.arch, total_memory: total, resource_policy: policy.resource_policy };
    stage = "residency_runtime";
    if (target.residency === "cold") await idle(runtime, abort.signal);
    else residentState(runtime, await api(runtime.endpoint, "/api/ps", null, abort.signal));
    stage = "frozen_manifest";
    const all = await manifests(target, target.suite, abort.signal);
    const frozen = command === "diagnose" ? all.filter(item => item.case.id === args[1]) : all;
    assert.ok(frozen.length > 0);
    const started = new Date().toISOString();
    const resources = { preload_checks: 0, inflight_checks: 0, minimum_preload_available_bytes: null, minimum_inflight_available_bytes: null, settle_wait_ms: 0 };
    if (target.residency === "reuse") Object.assign(resources, { cold_checks: 0, reload_checks: 0, reuse_checks: 0, minimum_loading_available_bytes: null, minimum_reuse_available_bytes: null });
    stage = "synthetic_cases";
    const outcome = await runCases(frozen, runtime, async id => {
      try {
        const raw = await guardedSend(signal => stagedCli(binary, [command === "diagnose" ? "diagnose" : "case", id, target.endpoint, target.model_alias, "--use-local-benchmark", "--guard-native-stages", "--suite", target.suite, "--backend", "ollama", "--residency", target.residency, "--candidate", target.candidate], { cwd: root, env },
          async (nativeStage, bounded) => {
            try {
              if (target.residency === "cold") await idle(runtime, bounded, nativeStage === "verify");
              assert.equal(digest(await identity(target, bounded)), digest(currentIdentity(runtime)));
            } catch { throw Object.assign(new Error("local runtime changed or busy"), { answerFailure: "runtime" }); }
            try {
              const ready = target.residency === "cold"
                ? await memoryReady(() => available(total), runtime.model_size, bounded)
                : await confirmResidentRequest(runtime, signal => api(runtime.endpoint, "/api/ps", null, signal), () => available(total), bounded);
              if (target.residency === "reuse") {
                resources[`${ready.state}_checks`]++;
                const field = ready.loading ? "minimum_loading_available_bytes" : "minimum_reuse_available_bytes";
                resources[field] = Math.min(resources[field] ?? total, ready.available);
              }
              resources.preload_checks++;
              resources.minimum_preload_available_bytes = Math.min(resources.minimum_preload_available_bytes ?? total, ready.available);
              resources.settle_wait_ms += ready.waited_ms;
            } catch (error) { throw Object.assign(new Error("local residency or reserve unconfirmed"), { answerFailure: error.answerFailure ?? "resource" }); }
          }, signal, 90000, ["guarded", "question-first", "evidence-bound"].includes(target.candidate) ? "guarded" : target.candidate === "question" ? "plan" : "select"),
          async () => {
            const free = await available(total); resources.inflight_checks++;
            resources.minimum_inflight_available_bytes = Math.min(resources.minimum_inflight_available_bytes ?? total, free);
            memoryBudget(free, runtime.model_size, false);
          }, abort.signal);
        try {
          if (target.residency === "cold") await idle(runtime, abort.signal, true);
          else residentState(runtime, await api(runtime.endpoint, "/api/ps", null, abort.signal));
          assert.equal(digest(await identity(target, abort.signal)), digest(currentIdentity(runtime)));
        } catch { throw Object.assign(new Error("local runtime changed or still busy"), { answerFailure: "runtime" }); }
        console.error(`Ollama ${target.suite}: ${id} completed`);
        return raw;
      } catch (error) {
        const fixed = /^ANSWER_FAILURE=(transport|protocol)$/m.exec(error.stderr ?? "");
        const protocolStage = error.answerFailureStage ?? /^ANSWER_PROTOCOL_STAGE=([a-z_]+)$/m.exec(error.stderr ?? "")?.[1];
        throw Object.assign(new Error("local answer unconfirmed"), { answerFailure: error.answerFailure ?? fixed?.[1], answerFailureStage: PROTOCOL_STAGES.includes(protocolStage) ? protocolStage : undefined });
      }
    }, command === "diagnose" ? ["guarded", "question-first", "evidence-bound"].includes(target.candidate) ? verifyGuardedDiagnostic : target.candidate === "question" ? verifyQuestionDiagnostic : verifyDiagnostic : undefined);
    const report = { schema: command === "diagnose" ? DIAGNOSTIC_REPORT_SCHEMA : policy.report_schema, run_id: randomUUID(), synthetic_only: true, suite: target.suite,
      ...(command === "diagnose" ? { diagnostic_only: true, private_execution_authorized: false } : {}),
      started_at: started, ended_at: new Date().toISOString(), runtime, resources, manifests: frozen, ...outcome,
      passed_cases: outcome.results.filter(result => result.quality_pass).length, total_cases: frozen.length };
    stage = "write_report";
    console.log(JSON.stringify({ report: await writeReport(directory, report), complete: report.complete,
      passed_cases: report.passed_cases, total_cases: report.total_cases, failure: report.failure, failure_stage: report.failure_stage, diagnostic_only: report.diagnostic_only,
      ...(command === "diagnose" ? { semantic_diagnostic: report.results[0]?.semantic_diagnostic } : {}), exit_code: report.exit_code }));
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
