import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
const defaultKey = "qwen3-4b";
export function modelKey(value = defaultKey) {
  if (![defaultKey, "qwen3-8b"].includes(value)) throw new Error("未知本地模型候选");
  return value;
}
export function modelCommandOptions(args) {
  if (args.length === 0) return defaultKey;
  if (args.length !== 2 || args[0] !== "--model") throw new Error("用法：--model qwen3-4b|qwen3-8b");
  return modelKey(args[1]);
}
export async function modelChoice(key = defaultKey) {
  modelKey(key);
  const base = await readFile(new URL("../infra/local-model-runtime.json", import.meta.url));
  let config = JSON.parse(base), bytes = base;
  if (key !== defaultKey) {
    const candidates = await readFile(new URL("../infra/local-model-candidates.json", import.meta.url));
    config = { ...config, ...JSON.parse(candidates)[key] };
    bytes = Buffer.concat([base, Buffer.from("\n"), candidates]);
  } else { config.load_budget_mib = 4096; config.context_size = 8192; }
  assert.equal(config.engine, "llama.cpp");
  assert.equal(config.endpoint, "http://127.0.0.1:11435");
  assert.equal(config.model_alias, key === defaultKey ? "qwen3:4b-q4_K_M" : "qwen3:8b-q4_K_M");
  assert.equal(config.load_budget_mib, key === defaultKey ? 4096 : 6144);
  assert.equal(config.context_size, key === defaultKey ? 8192 : 4096);
  assert.match(config.version, /^b[0-9]+$/);
  assert.match(config.model.filename, /^Qwen3-(4|8)B-Q4_K_M\.gguf$/);
  assert.match(config.model.sha256, /^[0-9a-f]{64}$/);
  assert.match(config.model.revision, /^[0-9a-f]{40}$/);
  return { key, config, configSha256: createHash("sha256").update(bytes).digest("hex") };
}
export function validateOwnerChoice(state, choice) {
  assert.equal(state.modelKey, choice.key);
  assert.equal(state.model, choice.config.model_alias);
  assert.equal(state.endpoint, choice.config.endpoint);
  assert.equal(state.configSha256, choice.configSha256);
  assert.equal(state.loadBudgetMiB, choice.config.load_budget_mib);
}
