// Project-owned foreground supervisor. No login service or automatic inference/restart.
import { spawn } from "node:child_process";
import { access, mkdir, open, readFile, realpath, unlink } from "node:fs/promises";
import { once } from "node:events";
import { fileURLToPath } from "node:url";
import { join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { readGpu, canLoad, canSend, watchBudget, reserveMiB } from "./local-model-budget.mjs";
import { modelChoice, modelCommandOptions, validateOwnerChoice } from "./local-model-choice.mjs";
import { benchmarkOptions } from "./local-benchmark-options.mjs";
const root = resolve(fileURLToPath(new URL("../", import.meta.url)));
const directory = join(root, ".local-model");
const binary = join(directory, "llama-server");
const statePath = join(directory, "supervisor.json");
const endpoint = "http://127.0.0.1:11435";
const command = process.argv[2];
async function owner() {
  let state;
  try { state = JSON.parse(await readFile(statePath, "utf8")); } catch (error) { if (error.code === "ENOENT") return null; throw error; }
  if (typeof state.root !== "string" || resolve(state.root) !== root || !Number.isSafeInteger(state.pid) || state.pid <= 1) throw new Error("项目模型状态不合法");
  try {
    const args = (await readFile(`/proc/${state.pid}/cmdline`, "utf8")).split("\0");
    const selected = modelCommandOptions(args.slice(3).filter(Boolean));
    if (selected !== (state.modelKey ?? "qwen3-4b")) throw new Error("模型进程与状态选择不一致");
    const cwd = await realpath(`/proc/${state.pid}/cwd`);
    if (resolve(cwd, args[1] ?? "") !== fileURLToPath(import.meta.url) || args[2] !== "start") throw new Error("状态进程不属于本项目，拒绝停止");
  } catch (error) { if (error.code === "ENOENT" || error.code === "ESRCH") return { ...state, stale: true }; throw error; }
  return state;
}
async function activeChoice(state) {
  const choice = await modelChoice(state.modelKey);
  validateOwnerChoice(state, choice);
  return choice;
}
async function readyToSend(choice) {
  const response = await fetch(`${endpoint}/props`, { signal: AbortSignal.timeout(2000) });
  if (!response.ok) throw new Error("本地模型状态不可核实");
  const state = await response.json();
  if (typeof state.is_sleeping !== "boolean") throw new Error("本地模型睡眠状态不可核实");
  if (!canSend(await readGpu(), state.is_sleeping, choice.config.load_budget_mib)) throw new Error("模型发送前显存预算不足；原授权尚未领取");
}
async function status() {
  const gpu = await readGpu(), supervisor = await owner();
  const choice = supervisor && !supervisor.stale ? await activeChoice(supervisor) : await modelChoice();
  const model = choice.config.model_alias;
  console.log(JSON.stringify({ ...gpu, reserveMiB, admission: canLoad(gpu, choice.config.load_budget_mib), endpoint, model, supervisor }, null, 2));
  if (!supervisor || supervisor.stale) { console.log("模型服务未运行"); return; }
  try {
    const response = await fetch(`${endpoint}/props`, { signal: AbortSignal.timeout(2000) });
    if (!response.ok) throw new Error("模型状态不可读取");
    const state = await response.json();
    if (typeof state.is_sleeping !== "boolean") throw new Error("模型状态不可核实");
    console.log(JSON.stringify({ sleeping: state.is_sleeping, sendAdmission: canSend(gpu, state.is_sleeping, choice.config.load_budget_mib) }, null, 2));
  } catch { console.log("模型服务未运行"); }
}
async function start() {
  const choice = await modelChoice(modelCommandOptions(process.argv.slice(3)));
  const { config } = choice, model = config.model_alias;
  const gpu = await readGpu();
  if (!canLoad(gpu, config.load_budget_mib)) throw new Error(`显存不足：需要 6 GiB 游戏余量、512 MiB 监控缓冲及 ${config.load_budget_mib} MiB 模型加载预算`);
  await access(binary);
  await access(join(directory, config.model.filename));
  const existing = await owner(); if (existing && !existing.stale) throw new Error("本项目模型服务已运行");
  if (existing?.stale) await unlink(statePath);
  try { await fetch(`${endpoint}/health`, { signal: AbortSignal.timeout(1500) }); throw new Error("11435 端口已有服务，拒绝接管"); }
  catch (error) { if (error.message === "11435 端口已有服务，拒绝接管") throw error; }
  await mkdir(directory, { recursive: true });
  const stateFile = await open(statePath, "wx", 0o600);
  await stateFile.writeFile(JSON.stringify({ pid: process.pid, root, endpoint, model, modelKey: choice.key, configSha256: choice.configSha256, loadBudgetMiB: config.load_budget_mib })); await stateFile.close();
  const log = await open(join(directory, "server.log"), "a", 0o600);
  const child = spawn(await realpath(binary), ["--model", join(directory, config.model.filename),
    "--alias", model, "--host", "127.0.0.1", "--port", "11435", "--ctx-size", String(config.context_size), "--parallel", "1",
    "--n-gpu-layers", "99", "--flash-attn", "on", "--cache-type-k", "q8_0", "--cache-type-v", "q8_0",
    "--batch-size", "256", "--ubatch-size", "128", "--fit", "on", "--fit-target", "6656",
    "--jinja", "--reasoning", "off", "--cors-origins", "http://127.0.0.1:11435", "--no-context-shift", "--sleep-idle-seconds", "30"
  ], { cwd: root, detached: true, stdio: ["ignore", log.fd, log.fd], env: {
    ...process.env, CUDA_VISIBLE_DEVICES: gpu.uuid,
  } });
  let closing = false, stopped = false, killTimer;
  const halt = () => {
    if (closing) return;
    closing = true;
    try { process.kill(-child.pid, "SIGTERM"); } catch { /* already exited */ }
    killTimer = setTimeout(() => {
      if (!stopped) { try { process.kill(-child.pid, "SIGKILL"); } catch { /* already exited */ } }
    }, 2000);
    killTimer.unref();
  };
  child.on("error", () => { stopped = true; }); child.on("close", () => { stopped = true; });
  process.on("SIGINT", halt); process.on("SIGTERM", halt);
  console.log(`本地模型服务 ${endpoint}，单模型/单并发；至少预留 6 GiB。Ctrl+C 或 make local-model-stop 停止。`);
  const monitor = () => watchBudget({ query: readGpu, gpuUuid: gpu.uuid,
    stopping: () => stopped || closing, wait: () => delay(500),
    violate: () => { console.error("无法维持显存余量，停止本项目模型；不会自动重启或重发。"); halt(); },
  });
  try {
    const result = await Promise.all([once(child, "close"), monitor()]);
    if (result[0][0] !== 0 && !closing) throw new Error("本地模型启动或运行失败，请检查项目私有日志");
  } finally {
    halt(); try { process.kill(-child.pid, "SIGKILL"); } catch { /* project group already exited */ }
    clearTimeout(killTimer);
    process.removeListener("SIGINT", halt); process.removeListener("SIGTERM", halt);
    await log.close(); await unlink(statePath).catch(() => {});
  }
}
async function stop() {
  const state = await owner(); if (!state) { console.log("本项目模型未运行"); return; }
  if (state.stale) { await unlink(statePath); console.log("已清理失效项目状态"); return; }
  process.kill(state.pid, "SIGTERM");
  for (let i = 0; i < 40; i++) { await delay(250); if (!(await owner())) { console.log("已停止本项目模型并释放其 GPU 资源"); return; } }
  throw new Error("停止尚未确认，请核对原项目进程");
}
async function runReview(probe = false, scoring = false) {
  const args = process.argv.slice(3);
  if (!probe && (args.length !== 2 || !args.every(v => /^[0-9a-f-]{36}$/i.test(v)))) throw new Error(`用法：make ${scoring ? "local-value" : "local-review"} OWNER=用户_UUID REQUEST=已批准的请求_UUID`);
  const state = await owner(); if (!state || state.stale) throw new Error("先显式启动受显存保护的本项目模型服务");
  const choice = await activeChoice(state), model = choice.config.model_alias;
  if (choice.key !== "qwen3-4b") throw new Error("候选模型仅用于合成基准，业务执行仍需默认模型及对应授权");
  await readyToSend(choice);
  const child = spawn("cargo", ["run", "--locked", "-p", "api-server", "--bin", scoring ? "local-value" : "local-review", "--", ...(probe ? ["probe", endpoint, model, "--use-local"] : ["run", ...args, endpoint, model, "--use-local"])], { cwd: root, env: process.env, stdio: "inherit" });
  const halt = () => child.kill("SIGTERM"); process.on("SIGINT", halt); process.on("SIGTERM", halt);
  try { const [code] = await once(child, "close"); if (code !== 0) throw new Error("本地执行未确认成功，请查询原请求，勿自动重发"); }
  finally { process.removeListener("SIGINT", halt); process.removeListener("SIGTERM", halt); }
}
async function benchmarkCase() {
  const { args, flags, model: selected } = benchmarkOptions(process.argv.slice(3));
  if (args.length !== 1 || !/^[a-z0-9_]{1,40}$/.test(args[0])) throw new Error("用法：benchmark-case 固定语料键 [--suite baseline|challenge] [--profile local-rss-v1|local-rss-v2|local-rss-v3]");
  const state = await owner(); if (!state || state.stale) throw new Error("先显式启动受显存保护的本项目模型服务");
  const choice = await activeChoice(state), model = choice.config.model_alias;
  if (choice.key !== selected) throw new Error("运行中的模型与请求候选不一致，拒绝发送");
  await readyToSend(choice);
  // Group runner builds this binary before any request; do not inherit DB/API credentials.
  const child = spawn(join(root, "target/debug/local-value-benchmark"), ["case", args[0], endpoint, model, "--use-local-benchmark", ...flags],
    { cwd: root, env: { PATH: process.env.PATH, HOME: process.env.HOME }, stdio: "inherit" });
  const halt = () => child.kill("SIGTERM"); process.on("SIGINT", halt); process.on("SIGTERM", halt);
  try { const [code] = await once(child, "close"); if (code !== 0) throw new Error("固定基准执行未确认，不自动重试"); }
  finally { process.removeListener("SIGINT", halt); process.removeListener("SIGTERM", halt); }
}
try {
  if (command === "start") await start(); else if (command === "status") await status(); else if (command === "stop") await stop();
  else if (command === "check") { const choice = await modelChoice(modelCommandOptions(process.argv.slice(3))); const gpu = await readGpu(); if (!canLoad(gpu, choice.config.load_budget_mib)) throw new Error("模型加载预算不足"); console.log("显存预算核对通过"); }
  else if (command === "review") await runReview();
  else if (command === "value") await runReview(false, true);
  else if (command === "probe") await runReview(true);
  else if (command === "benchmark-case") await benchmarkCase();
  else throw new Error("用法：node scripts/local-model.mjs start|status|stop|check|probe|review|value OWNER REQUEST|benchmark-case CASE");
} catch (error) { console.error(error.message); process.exitCode = 1; }
