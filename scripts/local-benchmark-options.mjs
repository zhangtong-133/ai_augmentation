// Fixed suites only; no user-provided corpus or text can enter synthetic benchmarks.
export function benchmarkOptions(input) {
  const args = [...input], values = {};
  while (args.length >= 2 && ["--profile", "--suite"].includes(args.at(-2))) {
    const value = args.pop(), name = args.pop();
    if (values[name] !== undefined) throw new Error("duplicate benchmark option");
    values[name] = value;
  }
  const profile = values["--profile"] ?? "local-rss-v2";
  const suite = values["--suite"] ?? "baseline";
  if (!["local-rss-v1", "local-rss-v2"].includes(profile) || !["baseline", "challenge"].includes(suite)) throw new Error("invalid benchmark option");
  return { args, profile, suite, flags: ["--profile", profile, "--suite", suite] };
}
