// Explicit private-answer admission. Receives target metadata only, never private content.
import { execFile } from "node:child_process";
import { lstat } from "node:fs/promises";
import { promisify } from "node:util";
import { admissionArguments, admit } from "./ollama-answer-admission-lib.mjs";
import { localMetadata } from "./ollama-answer-benchmark-lib.mjs";

const execute = promisify(execFile);
const benchmarkLock = new URL("../.local-model/benchmark.lock", import.meta.url);
const env = { PATH: process.env.PATH, HOME: process.env.HOME };
const abort = new AbortController();
const halt = () => abort.abort();
process.on("SIGINT", halt); process.on("SIGTERM", halt);
try {
  const input = admissionArguments(process.argv.slice(2));
  const signal = AbortSignal.any([abort.signal, AbortSignal.timeout(7000)]);
  const run = async (file, args, signal) => (await execute(file, args, {
    env, signal, timeout: 1000, killSignal: "SIGKILL", maxBuffer: 16384,
  })).stdout;
  const result = await admit(input.target, {
    platform: process.platform, arch: process.arch, signal, query: localMetadata,
    totalMemory: signal => run("/usr/sbin/sysctl", ["-n", "hw.memsize"], signal),
    memoryPressure: signal => run("/usr/bin/memory_pressure", ["-Q"], signal),
    benchmarkActive: async () => {
      try { await lstat(benchmarkLock); return true; }
      catch (error) { if (error.code === "ENOENT") return false; throw error; }
    },
  }, input.command);
  if (input.command === "inspect") console.log(JSON.stringify(result));
  else console.log(result.identity_sha256);
} catch {
  // Suppress tool output, request text and raw local metadata in the public error.
  console.error("Ollama 问答准入未确认；核对本机模型身份、驻留状态和统一内存余量。不会加载、卸载、推理或重试。");
  process.exitCode = 1;
} finally {
  process.removeListener("SIGINT", halt); process.removeListener("SIGTERM", halt);
}
