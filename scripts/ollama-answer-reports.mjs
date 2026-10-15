import assert from "node:assert/strict";
import { lstat, realpath } from "node:fs/promises";
import { join, resolve, sep } from "node:path";
import { readReport } from "./local-quality-report.mjs";
import { GUARDED_PROFILE, PROFILE, QUESTION_PROFILE, QUESTION_FIRST_PROFILE, REUSE_PROFILE, SUITES, parseOptions } from "./ollama-answer-benchmark-lib.mjs";

const reportCount = Object.keys(SUITES).length * 2;

export function comparisonFiles(args) {
  assert.equal(args.length, reportCount * 2 + 2);
  assert.equal(args[0], "--left");
  assert.equal(args[reportCount + 1], "--right");
  const left = args.slice(1, reportCount + 1), right = args.slice(reportCount + 2);
  assert.ok([...left, ...right].every(path => typeof path === "string" && path.length > 0 && !path.startsWith("-")));
  return { left, right };
}

// Restrict both gate and comparison to existing local quality reports. The shared reader
// opens without following a final symlink and bounds the actual bytes, not just stat.size.
export async function readQualityReports(files, base) {
  assert.ok(files.length === reportCount || files.length === reportCount * 2);
  for (const directory of [base, join(base, "quality")]) {
    const entry = await lstat(directory);
    assert.ok(entry.isDirectory() && !entry.isSymbolicLink());
  }
  const allowed = await realpath(join(base, "quality")), seen = new Set(), inputs = [];
  for (const file of files) {
    const path = resolve(file), entry = await lstat(path), actual = await realpath(path);
    assert.ok(entry.isFile() && !entry.isSymbolicLink() && actual.startsWith(`${allowed}${sep}`) && !seen.has(actual));
    seen.add(actual);
    inputs.push(await readReport(actual));
  }
  assert.equal(new Set(inputs.map(input => input.sha256)).size, inputs.length, "duplicate report bytes");
  return inputs;
}

export async function expectedManifests(reports, load) {
  const profile = reports[0].runtime.execution_profile;
  assert.ok([PROFILE, REUSE_PROFILE, QUESTION_PROFILE, GUARDED_PROFILE, QUESTION_FIRST_PROFILE].includes(profile));
  const target = parseOptions(["--endpoint", reports[0].runtime.endpoint, "--model", reports[0].runtime.model_alias,
    "--residency", profile === PROFILE ? "cold" : "reuse",
    "--candidate", profile === QUESTION_FIRST_PROFILE ? "question-first" : profile === GUARDED_PROFILE ? "guarded" : profile === QUESTION_PROFILE ? "question" : "independent"]);
  const expected = {};
  for (const suite of Object.keys(SUITES)) expected[suite] = await load(target, suite);
  return expected;
}
