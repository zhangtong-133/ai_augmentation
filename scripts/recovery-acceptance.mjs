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
import { exerciseLocalModel, seedApplication, startApplication, verifyApplication } from "./recovery-http.mjs";
import { checkOriginals, exportOriginals, objectOperation, restoreOriginals, verifyOriginals } from "./originals-recovery.mjs";
import { checkDeployment } from "./deployment-check.mjs";

const execute = promisify(execFile);
const flags = process.argv.slice(2);
if (flags.some(value => !["--local-model", "--objects", "--cached-images", "--storage-tests"].includes(value)) || new Set(flags).size !== flags.length || (flags.includes("--cached-images") && !flags.includes("--objects"))) throw new Error("仅支持显式 --local-model / --objects / --storage-tests 及配套 --cached-images 验收开关");
const native = process.argv.includes("--local-model");
const withObjects = process.argv.includes("--objects");
const directory = await mkdtemp(join(tmpdir(), "personal-ai-recovery-"));
let container;
let minio, network;
let sourceObjects = {}, targetObjects = {};
const applications = [];
async function docker(args) {
  try { return (await execute("docker", args, { timeout: args[0] === "build" ? 20 * 60 * 1000 : 120000, maxBuffer: 1048576 })).stdout.trim(); }
  catch { throw new Error(`隔离验收 Docker ${args[0]} 操作失败；不输出凭据`); }
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
  await writeFile(join(config, "config.json"), JSON.stringify({ auths: {}, cliPluginsExtraDirs: [...pluginDirs, join(originalConfig, "cli-plugins")] }), { mode: 0o600 });
  process.env.DOCKER_CONFIG = config; process.env.DOCKER_HOST = socket; delete process.env.DOCKER_CONTEXT;
  process.env.RECOVERY_TEST_PASSWORD = randomBytes(32).toString("hex");
  if (withObjects) {
    const cached = flags.includes("--cached-images");
    const minioImage = await docker(cached ? ["image", "inspect", "--format", "{{.Id}}", "quay.io/minio/minio:RELEASE.2025-04-22T22-12-26Z"] : ["build", "--quiet", "--target", "minio", join(root, "infra/minio")]);
    const mcImage = await docker(cached ? ["image", "inspect", "--format", "{{.Id}}", "quay.io/minio/mc:RELEASE.2025-04-16T18-13-26Z"] : ["build", "--quiet", "--target", "mc", join(root, "infra/minio")]);
    assert.match(minioImage, /^sha256:[a-f0-9]{64}$/); assert.match(mcImage, /^sha256:[a-f0-9]{64}$/);
    assert.match(await docker(["run", "--rm", "--pull", "never", "--entrypoint", "minio", minioImage, "--version"]), /RELEASE\.2025-04-22T22-12-26Z.*0d7408fc9969caf07de6a8c3a84f9fbb10a6739e/);
    assert.match(await docker(["run", "--rm", "--pull", "never", "--entrypoint", "mc", mcImage, "--version"]), /RELEASE\.2025-04-16T18-13-26Z.*b00526b153a31b36767991a4f5ce2cced435ee8e/);
    network = `personal-ai-recovery-${randomBytes(8).toString("hex")}`;
    await docker(["network", "create", "--label", "personal-ai.acceptance=recovery", network]);
    minio = await docker(["run", "--detach", "--network", network, "--network-alias", "minio", "--label", "personal-ai.acceptance=recovery", "--env", "MINIO_ROOT_USER=recovery", "--env", `MINIO_ROOT_PASSWORD=${process.env.RECOVERY_TEST_PASSWORD}`, "--publish", "127.0.0.1::9000", minioImage, "server", "/data"]);
    assert.match(minio, /^[a-f0-9]{64}$/);
    await docker(["run", "--rm", "--network", network, "--env", `RECOVERY_PASSWORD=${process.env.RECOVERY_TEST_PASSWORD}`, "--entrypoint", "/bin/sh", mcImage, "-c", 'attempt=0; until mc alias set test http://minio:9000 recovery "$RECOVERY_PASSWORD" >/dev/null 2>&1; do attempt=$((attempt+1)); [ "$attempt" -lt 60 ] || exit 1; sleep 1; done; mc mb test/source test/target test/conflict >/dev/null 2>&1']);
    const address = await docker(["port", minio, "9000/tcp"]); assert.match(address, /^127\.0\.0\.1:\d+$/);
    sourceObjects = { OBJECT_STORE_ENABLED: "true", OBJECT_STORE_ENDPOINT: `http://${address}`, OBJECT_STORE_BUCKET: "source", OBJECT_STORE_REGION: "us-east-1", OBJECT_STORE_ACCESS_KEY: "recovery", OBJECT_STORE_SECRET_KEY: process.env.RECOVERY_TEST_PASSWORD };
    targetObjects = { ...sourceObjects, OBJECT_STORE_BUCKET: "target" };
  }
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
  if (flags.includes("--storage-tests")) {
    await pgQuery(source,"CREATE DATABASE recovery_storage TEMPLATE template0;");
    const result=await execute("cargo",["test","--locked","-p","personal-ai-storage-postgres","--test","postgres","--","--ignored"],{cwd:root,timeout:600000,maxBuffer:4*1048576,env:{PATH:process.env.PATH,HOME:process.env.HOME,RUSTUP_TOOLCHAIN:process.env.RUSTUP_TOOLCHAIN,TEST_DATABASE_URL:databaseUrl("recovery_storage")}}).catch(error=>{ const failed=(error.stdout??"").split("\n").filter(line=>/^test .* FAILED$/.test(line)); throw new Error("隔离仓储验收失败："+failed.join("; ")); });
    console.log(result.stdout.split("\n").find(line=>line.startsWith("test result:"))??"PASS: disposable storage tests");
    const http=await execute("cargo",["test","--locked","-p","api-server","--lib","subscription_connection_tests","--","--ignored"],{cwd:root,timeout:600000,maxBuffer:4*1048576,env:{PATH:process.env.PATH,HOME:process.env.HOME,RUSTUP_TOOLCHAIN:process.env.RUSTUP_TOOLCHAIN,TEST_DATABASE_URL:databaseUrl("recovery_storage"),RSS_LOCAL_ENABLED:"true"}}).catch(error=>{throw new Error("隔离 HTTP 验收失败："+(error.stdout??"").split("\n").filter(line=>/^test .* FAILED$/.test(line)||/panicked at apps\//.test(line)||/^\s+(left|right): \d+$/.test(line)).join("; "));});
    console.log(http.stdout.split("\n").find(line=>line.startsWith("test result:"))??"PASS: disposable scoring HTTP tests");
  }
  const adminToken = randomBytes(32).toString("hex");
  const sourceApp = await startApplication(databaseUrl(source.database), adminToken, sourceObjects); applications.push(sourceApp);
  const fixture = await seedApplication(sourceApp);
  if (native) {
    await exerciseLocalModel(sourceApp, fixture);
    assert.equal(await pgQuery(source, `SELECT count(*) FROM learning_model_authorization_audit WHERE request_id='${fixture.nativeId}' AND event='sending';`), "1");
  } else console.log("SKIP: real local model; use make recovery-acceptance-local with an explicitly started protected server");
  await sourceApp.close();
  const expectedUsers = await pgQuery(source, "SELECT count(*) FROM users;");
  const expectedDocuments = await pgQuery(source, "SELECT count(*) FROM documents;");
  const diagnosisInput = { ...source, profile: "recovery" };
  const beforeDiagnosis = await pgQuery(source, await readFile(join(root, "tests/recovery/state.sql"), "utf8"));
  const sourceDiagnosis = await checkDeployment(diagnosisInput, sourceObjects);
  assert.equal(sourceDiagnosis.ready, false); assert.ok(sourceDiagnosis.counts.activeJobs > 0);
  assert.ok(sourceDiagnosis.issues.includes("recovery_activeJobs_remain"));
  assert.equal(await pgQuery(source, await readFile(join(root, "tests/recovery/state.sql"), "utf8")), beforeDiagnosis);
  const manifest = await backup(source, withObjects ? { captureOriginals: async (path, value) => {
    assert.equal(await pgQuery(source, "SELECT pg_try_advisory_xact_lock(7384920617);"), "f");
    return exportOriginals(path, value, sourceObjects);
  } } : {});
  if (withObjects) {
    assert.equal(manifest.externalOriginals, 1);
    const index = await verifyOriginals(source.directory);
    assert.equal((await stat(join(source.directory, "objects"))).mode & 0o777, 0o700);
    for (const file of ["index.json", index.objects[0].file]) assert.equal((await stat(join(source.directory, "objects", file))).mode & 0o777, 0o600);
    await assert.rejects(checkOriginals(source.directory, targetObjects), /缺失/);
    const conflictObjects = { ...sourceObjects, OBJECT_STORE_BUCKET: "conflict" };
    await objectOperation("create", index.objects[0].key, Buffer.from("different original"), conflictObjects);
    await assert.rejects(restoreOriginals(source.directory, conflictObjects), /覆盖/);
    assert.equal((await objectOperation("read", index.objects[0].key, undefined, conflictObjects)).toString(), "different original");
    await restoreOriginals(source.directory, targetObjects);
    await restoreOriginals(source.directory, targetObjects); // byte-identical resume never overwrites
    await checkOriginals(source.directory, targetObjects);
    await assert.rejects(objectOperation("create", index.objects[0].key, Buffer.from("must not overwrite"), targetObjects));
    await checkOriginals(source.directory, targetObjects);
    // The backup's transaction is already closed; protocol garbage collection lock must be released.
    assert.equal(await pgQuery(source, "SELECT pg_try_advisory_xact_lock(7384920617);"), "t");
    console.log("PASS: snapshot-bound real S3 original archive, private permissions, missing/conflicting target refusal and idempotent verified restore");
  }
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
  await assert.rejects(restore({ ...source, externalOriginalsReady: withObjects }));
  assert.equal(await pgQuery(source, "SELECT count(*) FROM users;"), expectedUsers);
  assert.equal(await pgQuery(source, "SELECT datallowconn FROM pg_database WHERE datname=current_database();"), "t");
  const target = { ...source, database: "recovery_target", externalOriginalsReady: withObjects };
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
  await assert.rejects(restore({ ...target, directory: external, externalOriginalsReady: false }), /S3/);
  // Valid checksum but invalid archive: partial SQL cannot commit; keep target offline.
  const invalid = Buffer.from("not a PostgreSQL archive");
  await writeFile(join(corrupt, "database.dump"), invalid);
  await writeFile(join(corrupt, "manifest.json"), JSON.stringify({ ...manifest, dump: { ...manifest.dump, bytes: invalid.length, sha256: createHash("sha256").update(invalid).digest("hex") } }));
  await assert.rejects(restore({ ...target, directory: corrupt }), /保持离线/);
  assert.equal(await pgQuery(source, "SELECT datallowconn FROM pg_database WHERE datname='recovery_target';"), "f");
  await pgQuery(source, 'ALTER DATABASE recovery_target ALLOW_CONNECTIONS true;');
  assert.equal(await pgQuery(target, "SELECT count(*) FROM pg_class WHERE relnamespace='public'::regnamespace;"), "0");
  if (withObjects) {
    const mismatch = join(directory, "reference-mismatch"); await cp(source.directory, mismatch, { recursive: true });
    const refs = JSON.parse(await readFile(join(mismatch, "originals.json"), "utf8"));
    refs[0].key = refs[0].key.slice(0, -36) + "00000000-0000-4000-8000-000000000009";
    const changed = Buffer.from(JSON.stringify(refs) + "\n");
    await writeFile(join(mismatch, "originals.json"), changed);
    await writeFile(join(mismatch, "manifest.json"), JSON.stringify({ ...manifest, originals: { ...manifest.originals, bytes: changed.length, sha256: createHash("sha256").update(changed).digest("hex") } }));
    await assert.rejects(restore({ ...target, directory: mismatch }), /保持离线/);
    assert.equal(await pgQuery(source, "SELECT datallowconn FROM pg_database WHERE datname='recovery_target';"), "f");
    await pgQuery(source, 'ALTER DATABASE recovery_target ALLOW_CONNECTIONS true;');
    assert.equal(await pgQuery(target, "SELECT count(*) FROM pg_class WHERE relnamespace='public'::regnamespace;"), "0");
    console.log("PASS: restored original references must match the archive snapshot before commit");
  }
  const result = await restore(target);
  assert.equal(result.quarantined, true);
  const state = JSON.parse(await pgQuery(target, await readFile(join(root, "tests/recovery/state.sql"), "utf8")));
  assert.equal(state.users, Number(expectedUsers)); assert.equal(state.documents, Number(expectedDocuments));
  for (const key of ["sessions", "activeMcp", "activeConfigurations", "unsettledMoney", "activeJobs"]) assert.equal(state[key], 0, key);
  assert.equal(state.occupiedMoney, 100); assert.equal(state.retainedMoney, 100);
  assert.equal(state.occupiedCalls, 1); assert.equal(state.toolCalls, 1);
  assert.deepEqual(state.learningStates, ["invalidated", "invalidated", ...(native ? ["succeeded"] : []), "unknown"]);
  assert.equal(state.unknownSending, 1); assert.equal(state.sendingAudits, 1);
  await assert.rejects(restore(target));
  assert.equal(await pgQuery(target, "SELECT count(*) FROM users;"), expectedUsers);
  console.log("PASS: empty-target transactional restore, corruption rollback, old-session/credential revocation, paused jobs and retained costs");
  const recoveryDiagnosis = await checkDeployment({ ...target, profile: "recovery", ...(withObjects ? { originals: source.directory } : {}) }, targetObjects);
  assert.equal(recoveryDiagnosis.ready, true); assert.ok(recoveryDiagnosis.counts.unknownModelRequests > 0);
  assert.equal(JSON.stringify(recoveryDiagnosis).includes(fixture.owner.id), false);
  assert.equal(JSON.stringify(recoveryDiagnosis).includes(fixture.owner.email), false);
  assert.deepEqual(JSON.parse(await pgQuery(target, await readFile(join(root, "tests/recovery/state.sql"), "utf8"))), state);
  await pgQuery(target, "UPDATE _sqlx_migrations SET success=false WHERE version=(SELECT min(version) FROM _sqlx_migrations);");
  const invalidDiagnosis = await checkDeployment({ ...target, profile: "current" }, targetObjects);
  assert.equal(invalidDiagnosis.ready, false); assert.ok(invalidDiagnosis.issues.includes("migration_mismatch"));
  await pgQuery(target, "UPDATE _sqlx_migrations SET success=true WHERE NOT success;");
  if (withObjects) {
    const unchecked = await checkDeployment({ ...target, profile: "recovery" }, targetObjects);
    assert.equal(unchecked.ready, false); assert.ok(unchecked.issues.includes("originals_not_verified"));
    const conflict = await checkDeployment({ ...target, profile: "recovery", originals: source.directory }, { ...targetObjects, OBJECT_STORE_BUCKET: "conflict" });
    assert.equal(conflict.ready, false); assert.ok(conflict.issues.includes("originals_mismatch_or_unavailable"));
  }
  console.log("PASS: read-only deployment diagnosis rejects active recovery state, wrong migrations and unverified/conflicting originals, and accepts quarantined target without changing data");
  const restoredApp = await startApplication(databaseUrl(target.database), adminToken, targetObjects); applications.push(restoredApp);
  await verifyApplication(restoredApp, fixture);
  if (native) assert.equal(await pgQuery(target, `SELECT count(*) FROM learning_model_authorization_audit WHERE request_id='${fixture.nativeId}' AND event='sending';`), "1");
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
  if (minio && /^[a-f0-9]{64}$/.test(minio)) {
    try { assert.equal(await docker(["inspect", "--format", '{{index .Config.Labels "personal-ai.acceptance"}}', minio]), "recovery"); await docker(["rm", "--force", "--volumes", minio]); }
    catch { console.error("隔离原文容器清理未确认"); process.exitCode = 1; }
  }
  if (network) {
    try { assert.equal(await docker(["network", "inspect", "--format", '{{index .Labels "personal-ai.acceptance"}}', network]), "recovery"); await docker(["network", "rm", network]); }
    catch { console.error("隔离原文网络清理未确认"); process.exitCode = 1; }
  }
  await rm(directory, { recursive: true, force: true });
}
