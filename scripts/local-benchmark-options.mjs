import { modelKey } from "./local-model-choice.mjs";
// Fixed suites only; no user-provided corpus or text can enter synthetic benchmarks.
export function benchmarkOptions(input) {
  const args = [...input], values = {};
  while (args.length >= 2 && ["--profile", "--suite", "--model"].includes(args.at(-2))) {
    const value = args.pop(), name = args.pop();
    if (values[name] !== undefined) throw new Error("duplicate benchmark option");
    values[name] = value;
  }
  const profile = values["--profile"] ?? "local-rss-v4";
  const suite = values["--suite"] ?? "baseline";
  if (!["local-rss-v1", "local-rss-v2", "local-rss-v3", "local-rss-v4"].includes(profile) || !["baseline", "challenge", "regression", "order", "public_calibration", "public_holdout"].includes(suite)) throw new Error("invalid benchmark option");
  return { args, profile, suite, model: modelKey(values["--model"]), flags: ["--profile", profile, "--suite", suite] };
}
