// Disposable PostgreSQL only; never reads .env or targets a regular Compose stack.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { randomBytes } from "node:crypto";
import { createReadStream } from "node:fs";
import { mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { homedir, tmpdir } from "node:os";
import { setTimeout as delay } from "node:timers/promises";
import { backup, expectedMigrations, localDocker, pgProcess, pgQuery, root, verify } from "./recovery-lib.mjs";

const execute = promisify(execFile);
const directory = await mkdtemp(join(tmpdir(), "personal-ai-recovery-"));
let container;
async function docker(args) {
  try { return (await execute("docker", args, { timeout: 120000, maxBuffer: 1048576 })).stdout.trim(); }
  catch { throw new Error("隔离验收 Docker 操作失败；不输出凭据"); }
}
try {
  await localDocker();
  const socket = process.env.DOCKER_CONTEXT || !process.env.DOCKER_HOST
    ? await docker(["context", "inspect", "--format", "{{.Endpoints.docker.Host}}"])
    : process.env.DOCKER_HOST;
  const originalConfig = process.env.DOCKER_CONFIG || join(homedir(), ".docker");
  let pluginDirs = [];
  try { pluginDirs = JSON.parse(await readFile(join(originalConfig, "config.json"), "utf8")).cliPluginsExtraDirs ?? []; }
  catch (error) { if (error.code !== "ENOENT") throw new Error("无法读取 Docker 插件配置"); }
  const config = join(directory, "docker");
  await (await import("node:fs/promises")).mkdir(config, { mode: 0o700 });
  await writeFile(join(config, "config.json"), JSON.stringify({ auths: {}, cliPluginsExtraDirs: pluginDirs }), { mode: 0o600 });
  process.env.DOCKER_CONFIG = config; process.env.DOCKER_HOST = socket; delete process.env.DOCKER_CONTEXT;
  process.env.RECOVERY_TEST_PASSWORD = randomBytes(32).toString("hex");
  container = await docker(["run", "--detach", "--name", `personal-ai-recovery-${randomBytes(8).toString("hex")}`,
    "--label", "personal-ai.acceptance=recovery", "--env", "POSTGRES_PASSWORD=" + process.env.RECOVERY_TEST_PASSWORD,
    "--env", "POSTGRES_USER=recovery", "--env", "POSTGRES_DB=recovery_source", "postgres:16-alpine"]);
  assert.match(container, /^[a-f0-9]{64}$/);
  const source = { container, database: "recovery_source", directory: join(directory, "backup") };
  let available = false;
  for (let i = 0; i < 60; i++) {
    try { await pgQuery(source, "SELECT 1;"); available = true; break; } catch { await delay(1000); }
  }
  assert.equal(available, true, "isolated PostgreSQL readiness");
  const migrations = await expectedMigrations();
  const names = (await readdir(join(root, "crates/storage-postgres/migrations"))).filter(v => /^\d{4}_.*\.sql$/.test(v)).sort();
  let schema = "BEGIN; CREATE TABLE _sqlx_migrations(version bigint PRIMARY KEY, description text NOT NULL, installed_on timestamptz NOT NULL DEFAULT now(), success boolean NOT NULL, checksum bytea NOT NULL, execution_time bigint NOT NULL);\n";
  for (let i = 0; i < names.length; i++) {
    schema += await readFile(join(root, "crates/storage-postgres/migrations", names[i]), "utf8");
    schema += `\nINSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(${migrations[i].version},'recovery fixture',true,decode('${migrations[i].checksum}','hex'),0);\n`;
  }
  await pgQuery(source, schema + "COMMIT;");
  await pgQuery(source, "INSERT INTO users(id,email,display_name) VALUES('00000000-0000-4000-8000-000000000001','synthetic@recovery.example','恢复测试');");
  const manifest = await backup(source);
  assert.deepEqual(await verify(source.directory), manifest);
  assert.equal((await stat(source.directory)).mode & 0o777, 0o700);
  for (const file of ["database.dump", "manifest.json"]) assert.equal((await stat(join(source.directory, file))).mode & 0o777, 0o600);
  const { child, done } = pgProcess(source, "pg_restore", ["--list"]);
  let contents = ""; child.stdout.on("data", chunk => { contents += chunk; });
  createReadStream(join(source.directory, "database.dump")).pipe(child.stdin);
  await done;
  assert.match(contents, /TABLE DATA public users/);
  assert.match(contents, /TABLE DATA public learning_model_authorizations/);
  await assert.rejects(backup(source)); // never replace an existing bundle
  assert.deepEqual(await verify(source.directory), manifest);
  console.log("PASS: real PostgreSQL 16 snapshot export, full archive, private permissions and refusal to overwrite");
} catch (error) {
  console.error(error.code ? "隔离恢复验收文件操作失败" : error.message); process.exitCode = 1;
} finally {
  if (container && /^[a-f0-9]{64}$/.test(container)) {
    try {
      assert.equal(await docker(["inspect", "--format", '{{index .Config.Labels "personal-ai.acceptance"}}', container]), "recovery");
      await docker(["rm", "--force", "--volumes", container]);
    } catch { console.error("隔离验收资源清理未确认"); process.exitCode = 1; }
  }
  await rm(directory, { recursive: true, force: true });
}
