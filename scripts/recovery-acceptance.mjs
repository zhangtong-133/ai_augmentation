// Disposable PostgreSQL only; never reads .env or targets a regular Compose stack.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { createHash, randomBytes } from "node:crypto";
import { createReadStream } from "node:fs";
import { cp, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { homedir, tmpdir } from "node:os";
import { setTimeout as delay } from "node:timers/promises";
import { once } from "node:events";
import { backup, expectedMigrations, localDocker, pgProcess, pgQuery, root, verify } from "./recovery-lib.mjs";
import { restore } from "./recovery-restore.mjs";
import { seedApplication, startApplication, verifyApplication } from "./recovery-http.mjs";

const execute = promisify(execFile);
const directory = await mkdtemp(join(tmpdir(), "personal-ai-recovery-"));
let container;
const applications = [];
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
  await mkdir(config, { mode: 0o700 });
  await writeFile(join(config, "config.json"), JSON.stringify({ auths: {}, cliPluginsExtraDirs: pluginDirs }), { mode: 0o600 });
  process.env.DOCKER_CONFIG = config; process.env.DOCKER_HOST = socket; delete process.env.DOCKER_CONTEXT;
  process.env.RECOVERY_TEST_PASSWORD = randomBytes(32).toString("hex");
  container = await docker(["run", "--detach", "--name", `personal-ai-recovery-${randomBytes(8).toString("hex")}`,
    "--label", "personal-ai.acceptance=recovery", "--env", "POSTGRES_PASSWORD=" + process.env.RECOVERY_TEST_PASSWORD,
    "--env", "POSTGRES_USER=recovery", "--env", "POSTGRES_DB=recovery_source", "--publish", "127.0.0.1::5432", "postgres:16-alpine"]);
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
  await pgQuery(source, await readFile(join(root, "tests/recovery/active.sql"), "utf8"));
  const address = await docker(["port", container, "5432/tcp"]); assert.match(address, /^127\.0\.0\.1:\d+$/);
  const databaseUrl = name => `postgres://recovery:${process.env.RECOVERY_TEST_PASSWORD}@${address}/${name}`;
  const adminToken = randomBytes(32).toString("hex");
  const sourceApp = await startApplication(databaseUrl(source.database), adminToken); applications.push(sourceApp);
  const fixture = await seedApplication(sourceApp);
  await sourceApp.close();
  const expectedUsers = await pgQuery(source, "SELECT count(*) FROM users;");
  const expectedDocuments = await pgQuery(source, "SELECT count(*) FROM documents;");
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
  // Refusal must leave both existing data and connection policy unchanged.
  await assert.rejects(restore(source));
  assert.equal(await pgQuery(source, "SELECT count(*) FROM users;"), expectedUsers);
  assert.equal(await pgQuery(source, "SELECT datallowconn FROM pg_database WHERE datname=current_database();"), "t");
  const target = { ...source, database: "recovery_target" };
  await pgQuery(source, 'CREATE DATABASE recovery_target TEMPLATE template0;');
  const busy = pgProcess(target, "psql", ["-X", "-qAt", "--set", "ON_ERROR_STOP=1"]);
  busy.child.stdin.write("SELECT 1;\n");
  await Promise.race([once(busy.child.stdout, "data"), busy.done.then(() => { throw new Error("occupied target connection lost"); })]);
  try { await assert.rejects(restore(target), /前置检查/); }
  finally { busy.child.stdin.end(); await busy.done; }
  assert.equal(await pgQuery(target, "SELECT datallowconn FROM pg_database WHERE datname=current_database();"), "t");
  const corrupt = join(directory, "corrupt"); await cp(source.directory, corrupt, { recursive: true });
  await writeFile(join(corrupt, "database.dump"), "damaged");
  await assert.rejects(restore({ ...target, directory: corrupt }), /SHA-256/);
  assert.equal(await pgQuery(target, "SELECT count(*) FROM pg_class WHERE relnamespace='public'::regnamespace;"), "0");
  const external = join(directory, "external"); await cp(source.directory, external, { recursive: true });
  await writeFile(join(external, "manifest.json"), JSON.stringify({ ...manifest, externalOriginals: 1 }));
  await assert.rejects(restore({ ...target, directory: external }), /S3/);
  // Valid checksum but invalid archive: partial SQL cannot commit; keep target offline.
  const invalid = Buffer.from("not a PostgreSQL archive");
  await writeFile(join(corrupt, "database.dump"), invalid);
  await writeFile(join(corrupt, "manifest.json"), JSON.stringify({ ...manifest, dump: { ...manifest.dump, bytes: invalid.length, sha256: createHash("sha256").update(invalid).digest("hex") } }));
  await assert.rejects(restore({ ...target, directory: corrupt }), /保持离线/);
  assert.equal(await pgQuery(source, "SELECT datallowconn FROM pg_database WHERE datname='recovery_target';"), "f");
  await pgQuery(source, 'ALTER DATABASE recovery_target ALLOW_CONNECTIONS true;');
  assert.equal(await pgQuery(target, "SELECT count(*) FROM pg_class WHERE relnamespace='public'::regnamespace;"), "0");
  const result = await restore(target);
  assert.equal(result.quarantined, true);
  const state = JSON.parse(await pgQuery(target, await readFile(join(root, "tests/recovery/state.sql"), "utf8")));
  assert.equal(state.users, Number(expectedUsers)); assert.equal(state.documents, Number(expectedDocuments));
  for (const key of ["sessions", "activeMcp", "activeConfigurations", "unsettledMoney", "activeJobs"]) assert.equal(state[key], 0, key);
  assert.equal(state.occupiedMoney, 100); assert.equal(state.retainedMoney, 100);
  assert.equal(state.occupiedCalls, 1); assert.equal(state.toolCalls, 1);
  assert.deepEqual(state.learningStates, ["invalidated", "invalidated", "unknown"]);
  assert.equal(state.unknownSending, 1); assert.equal(state.sendingAudits, 1);
  await assert.rejects(restore(target));
  assert.equal(await pgQuery(target, "SELECT count(*) FROM users;"), expectedUsers);
  console.log("PASS: empty-target transactional restore, corruption rollback, old-session/credential revocation, paused jobs and retained costs");
  const restoredApp = await startApplication(databaseUrl(target.database), adminToken); applications.push(restoredApp);
  await verifyApplication(restoredApp, fixture);
} catch (error) {
  console.error(error.code && error.code !== "ERR_ASSERTION" ? "隔离恢复验收文件操作失败" : error.message); process.exitCode = 1;
} finally {
  await Promise.allSettled(applications.map(app => app.close()));
  if (container && /^[a-f0-9]{64}$/.test(container)) {
    try {
      assert.equal(await docker(["inspect", "--format", '{{index .Config.Labels "personal-ai.acceptance"}}', container]), "recovery");
      await docker(["rm", "--force", "--volumes", container]);
    } catch { console.error("隔离验收资源清理未确认"); process.exitCode = 1; }
  }
  await rm(directory, { recursive: true, force: true });
}
