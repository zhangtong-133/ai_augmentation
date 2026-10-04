import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { createHash } from "node:crypto";
import { createReadStream, createWriteStream } from "node:fs";
import { lstat, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";
import { join, resolve } from "node:path";

const execute = promisify(execFile);
export const root = fileURLToPath(new URL("../", import.meta.url));
const format = "personal-ai-postgres-backup-v1";
export function parseOptions(args, command) {
  const options = {};
  for (let i = 0; i < args.length; i += 2) {
    const key = args[i];
    if (!["--container", "--database", "--directory", ...(command === "restore" ? ["--external-originals-ready"] : [])].includes(key) || !args[i + 1] || options[key]) throw new Error("参数缺失、重复或不支持");
    options[key] = args[i + 1];
  }
  if (!options["--directory"]) throw new Error("需要 --directory");
  if (command !== "verify") {
    if (!/^[a-f0-9]{64}$/.test(options["--container"] ?? "")) throw new Error("需要明确的完整 Docker 容器 ID");
    const database = options["--database"];
    if (!/^[a-z][a-z0-9_]{0,62}$/.test(database ?? "") || ["postgres", "template0", "template1"].includes(database)) throw new Error("需要明确的独立应用数据库名称");
  } else if (Object.keys(options).length !== 1) throw new Error("离线校验只接受 --directory");
  if (options["--external-originals-ready"] && options["--external-originals-ready"] !== "true") throw new Error("外部原文确认只接受 true");
  return { container: options["--container"], database: options["--database"], directory: resolve(options["--directory"]), externalOriginalsReady: options["--external-originals-ready"] === "true" };
}
export async function localDocker() {
  let host;
  try {
    host = process.env.DOCKER_CONTEXT || !process.env.DOCKER_HOST
      ? (await execute("docker", ["context", "inspect", "--format", "{{.Endpoints.docker.Host}}"], { timeout: 10000 })).stdout.trim()
      : process.env.DOCKER_HOST;
  } catch { throw new Error("无法核实 Docker 连接"); }
  if (!host.startsWith("unix:///")) throw new Error("备份恢复仅支持本机 Unix Docker socket");
}
export async function expectedMigrations() {
  const directory = join(root, "crates/storage-postgres/migrations");
  const names = (await readdir(directory)).filter(name => /^\d{4}_[a-z0-9_]+\.sql$/.test(name)).sort();
  return Promise.all(names.map(async name => ({ version: Number(name.slice(0, 4)), checksum: createHash("sha384").update(await readFile(join(directory, name))).digest("hex") })));
}
export function matchingMigrations(actual, expected) {
  return Array.isArray(actual) && actual.length === expected.length && actual.every((item, i) =>
    item && Object.keys(item).sort().join(",") === "checksum,version" && item.version === expected[i].version && item.checksum === expected[i].checksum);
}
export function validateManifest(value) {
  const keys = "createdAt,dump,externalOriginals,format,migrations,postgresMajor";
  if (!value || ![keys, "createdAt,dump,externalOriginals,format,migrations,originals,postgresMajor"].includes(Object.keys(value).sort().join(",")) || value.format !== format || value.postgresMajor !== 16 ||
      typeof value.createdAt !== "string" || !/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(value.createdAt) || !Number.isFinite(Date.parse(value.createdAt)) ||
      !Number.isSafeInteger(value.externalOriginals) || value.externalOriginals < 0 || !Array.isArray(value.migrations) || !value.migrations.length ||
      !value.dump || Object.keys(value.dump).sort().join(",") !== "bytes,file,sha256" || value.dump.file !== "database.dump" ||
      !Number.isSafeInteger(value.dump.bytes) || value.dump.bytes <= 0 || !/^[a-f0-9]{64}$/.test(value.dump.sha256)) throw new Error("备份清单格式或版本不支持");
  if (Object.hasOwn(value, "originals") && (!value.originals || Object.keys(value.originals).sort().join(",") !== "bytes,file,sha256" || value.originals.file !== "originals.json" || !Number.isSafeInteger(value.originals.bytes) || value.originals.bytes < 1 || value.originals.bytes > 1048576 || !/^[a-f0-9]{64}$/.test(value.originals.sha256))) throw new Error("原文引用清单不合法");
  return value;
}
async function regular(path) {
  const info = await lstat(path);
  if (!info.isFile()) throw new Error("备份文件必须是普通文件，拒绝符号链接");
  return info;
}
export async function digestFile(path) {
  const info = await regular(path), digest = createHash("sha256");
  for await (const chunk of createReadStream(path)) digest.update(chunk);
  return { bytes: info.size, sha256: digest.digest("hex") };
}
export async function verify(directory) {
  const manifestPath = join(directory, "manifest.json");
  if ((await regular(manifestPath)).size > 65536) throw new Error("备份清单过大");
  let manifest;
  try { manifest = validateManifest(JSON.parse(await readFile(manifestPath, "utf8"))); }
  catch { throw new Error("备份清单格式或版本不支持"); }
  if (!matchingMigrations(manifest.migrations, await expectedMigrations())) throw new Error("备份迁移与当前代码不同，拒绝恢复");
  const digest = await digestFile(join(directory, "database.dump"));
  if (digest.bytes !== manifest.dump.bytes || digest.sha256 !== manifest.dump.sha256) throw new Error("备份大小或 SHA-256 不匹配");
  if (manifest.originals) {
    const refs = await digestFile(join(directory, "originals.json"));
    if (refs.bytes !== manifest.originals.bytes || refs.sha256 !== manifest.originals.sha256) throw new Error("原文引用大小或 SHA-256 不匹配");
  }
  return manifest;
}

// The container is the explicit database endpoint; credentials stay inside it.
// All executable text is fixed. Values are passed as arguments, never interpolated.
export function pgProcess(target, tool, args = []) {
  if (!["psql", "pg_dump", "pg_restore"].includes(tool)) throw new Error("不支持的 PostgreSQL 工具");
  const child = spawn("docker", ["exec", "-i", "--env", `RECOVERY_DATABASE=${target.database}`, target.container,
    "sh", "-c", 'export PGPASSWORD="${POSTGRES_PASSWORD:?}" PGHOST=127.0.0.1 PGPORT=5432 PGUSER="${POSTGRES_USER:-postgres}" PGDATABASE="$RECOVERY_DATABASE"; exec "$@"',
    "recovery", tool, ...args, ...(tool === "psql" ? ["--set", "VERBOSITY=sqlstate"] : [])], { cwd: root, stdio: ["pipe", "pipe", "pipe"] });
  let diagnostic = "", sqlState;
  child.stderr.on("data", chunk => {
    diagnostic = (diagnostic + chunk).slice(-256);
    sqlState = diagnostic.match(/ERROR:\s+([0-9A-Z]{5})\b/)?.[1] ?? sqlState;
  });
  child.stdin.on("error", () => {});
  const timer = setTimeout(() => child.kill("SIGKILL"), 20 * 60 * 1000);
  const done = new Promise((resolveDone, reject) => {
    child.on("error", () => { clearTimeout(timer); reject(new Error(`${tool} 无法启动`)); });
    child.on("close", code => { clearTimeout(timer); code === 0 ? resolveDone() : reject(new Error(`${tool} 执行失败${sqlState ? `（SQLSTATE ${sqlState}）` : ""}；未输出连接凭据或数据库正文`)); });
  });
  // A stream can fail before the caller awaits process completion.
  done.catch(() => {});
  return { child, done };
}
export async function pgQuery(target, sql) {
  const { child, done } = pgProcess(target, "psql", ["-X", "-qAt", "--set", "ON_ERROR_STOP=1"]);
  let output = "", bytes = 0;
  child.stdout.on("data", chunk => { bytes += chunk.length; if (bytes > 1048576) child.kill("SIGKILL"); else output += chunk; });
  child.stdin.end(sql);
  await done;
  if (bytes > 1048576) throw new Error("查询元数据超过限制");
  return output.trim();
}
const metadataSql = `BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL lock_timeout='5s';
DO $$ BEGIN PERFORM pg_advisory_xact_lock_shared(7384920617); END $$;
SELECT jsonb_build_object('snapshot',pg_export_snapshot(),'postgresMajor',current_setting('server_version_num')::int/10000,
 'failedMigrations',(SELECT count(*) FROM _sqlx_migrations WHERE NOT success),
 'migrations',(SELECT jsonb_agg(jsonb_build_object('version',version,'checksum',encode(checksum,'hex')) ORDER BY version) FROM _sqlx_migrations WHERE success),
 'externalOriginals',(SELECT count(*) FROM documents WHERE original_object_key IS NOT NULL));\n`;
async function snapshot(target) {
  const { child, done } = pgProcess(target, "psql", ["-X", "-qAt", "--set", "ON_ERROR_STOP=1"]);
  let output = "";
  const metadata = new Promise((resolveMetadata, reject) => {
    child.stdout.on("data", chunk => {
      output += chunk;
      if (output.length > 65536) { child.kill("SIGKILL"); reject(new Error("快照元数据超过限制")); }
      if (output.includes("\n")) {
        try { resolveMetadata(JSON.parse(output.split("\n")[0])); } catch { reject(new Error("快照元数据不合法")); }
      }
    });
    child.on("close", () => reject(new Error("无法读取一致性快照")));
    child.on("error", () => reject(new Error("无法读取一致性快照")));
  });
  child.stdin.write(metadataSql);
  try {
    const state = await metadata;
    return { state, close: async () => { child.stdin.end("ROLLBACK;\n"); await done; } };
  } catch (error) { child.kill("SIGKILL"); await done.catch(() => {}); throw error; }
}
export async function backup(target, { captureOriginals } = {}) {
  target = parseOptions(["--container", target.container, "--database", target.database, "--directory", target.directory], "backup");
  await localDocker();
  const transaction = await snapshot(target);
  let created = false;
  try {
    const state = transaction.state;
    if (state.postgresMajor !== 16 || state.failedMigrations !== 0 || !matchingMigrations(state.migrations, await expectedMigrations()) ||
        !/^[0-9A-F]{8}-[0-9A-F]{8}-[0-9]+$/.test(state.snapshot)) throw new Error("备份需要 PostgreSQL 16 与当前代码的完整迁移");
    await mkdir(target.directory, { mode: 0o700 }); created = true;
    const path = join(target.directory, "database.dump");
    const { child, done } = pgProcess(target, "pg_dump", ["--format=custom", "--no-owner", "--no-acl", `--snapshot=${state.snapshot}`]);
    child.stdin.end();
    try { await pipeline(child.stdout, createWriteStream(path, { flags: "wx", mode: 0o600 })); await done; }
    catch (error) { child.kill("SIGKILL"); await done.catch(() => {}); throw error; }
    const digest = await digestFile(path);
    const manifest = { format, createdAt: new Date().toISOString(), postgresMajor: state.postgresMajor,
      migrations: state.migrations, externalOriginals: state.externalOriginals, dump: { file: "database.dump", ...digest } };
    if (state.externalOriginals > 1000) throw new Error("原文备份首版最多支持 1000 个引用，拒绝不完整导出");
    const refs = await pgQuery(target, `BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; SET TRANSACTION SNAPSHOT '${state.snapshot}'; SELECT COALESCE(jsonb_agg(jsonb_build_object('key',original_object_key,'sourceType',source_type) ORDER BY original_object_key),'[]'::jsonb) FROM documents WHERE original_object_key IS NOT NULL; ROLLBACK;`);
    const entries = JSON.parse(refs);
    if (!Array.isArray(entries) || entries.length !== state.externalOriginals) throw new Error("原文引用与数据库快照不一致");
    await writeFile(join(target.directory, "originals.json"), JSON.stringify(entries) + "\n", { flag: "wx", mode: 0o600 });
    manifest.originals = { file: "originals.json", ...await digestFile(join(target.directory, "originals.json")) };
    if (captureOriginals) await captureOriginals(target.directory, manifest);
    validateManifest(manifest);
    await writeFile(join(target.directory, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n", { flag: "wx", mode: 0o600 });
    await verify(target.directory);
    return manifest;
  } catch (error) { if (created) await rm(target.directory, { recursive: true, force: true }); throw error; }
  finally { await transaction.close(); }
}
