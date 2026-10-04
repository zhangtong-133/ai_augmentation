import { chmod, lstat, mkdir, mkdtemp, open } from "node:fs/promises";
import { join } from "node:path";
// Fresh private reports only; callers pass bounded metadata, never user/model text.
export async function newReportDirectory(base, kind) {
  if (!["quality", "observations"].includes(kind)) throw new Error("invalid report kind");
  for (const directory of [base, join(base, kind)]) {
    await mkdir(directory, { recursive: true, mode: 0o700 });
    const entry = await lstat(directory);
    if (!entry.isDirectory() || entry.isSymbolicLink()) throw new Error("unsafe report directory");
    await chmod(directory, 0o700);
  }
  const directory = await mkdtemp(join(base, kind, "run-"));
  await chmod(directory, 0o700);
  return directory;
}
export async function writeReport(directory, report) {
  const data = `${JSON.stringify(report, null, 2)}\n`;
  if (Buffer.byteLength(data) > 65536) throw new Error("report exceeds metadata limit");
  const file = join(directory, "report.json");
  const handle = await open(file, "wx", 0o600);
  try { await handle.writeFile(data); } finally { await handle.close(); }
  return file;
}
