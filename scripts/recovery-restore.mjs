import { createReadStream } from "node:fs";
import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { Transform } from "node:stream";
import { pipeline } from "node:stream/promises";
import { join } from "node:path";
import { localDocker, parseOptions, pgProcess, pgQuery, root, verify } from "./recovery-lib.mjs";

const emptyTarget = `DO $$ BEGIN
 IF current_setting('server_version_num')::int/10000<>16 THEN RAISE EXCEPTION 'PostgreSQL version mismatch'; END IF;
 IF EXISTS(SELECT 1 FROM pg_namespace WHERE nspname NOT IN ('public','pg_catalog','pg_toast','information_schema')
  AND nspname !~ '^pg_(toast_)?temp_[0-9]+$')
 OR EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public')
 OR EXISTS(SELECT 1 FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public')
 OR EXISTS(SELECT 1 FROM pg_type t JOIN pg_namespace n ON n.oid=t.typnamespace WHERE n.nspname='public')
 OR EXISTS(SELECT 1 FROM pg_extension WHERE extname<>'plpgsql')
 OR EXISTS(SELECT 1 FROM pg_largeobject_metadata) THEN RAISE EXCEPTION 'restore target is not empty'; END IF;
 IF EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid()
  AND backend_type='client backend') THEN RAISE EXCEPTION 'restore target has other connections'; END IF;
 END $$;\n`;

export async function restore(target) {
  target = parseOptions(["--container", target.container, "--database", target.database, "--directory", target.directory,
    ...(target.externalOriginalsReady ? ["--external-originals-ready", "true"] : [])], "restore");
  const manifest = await verify(target.directory); // no database mutation before validation
  let referenceCheck = "";
  if (manifest.originals) {
    const refs = await readFile(join(target.directory, "originals.json"));
    if (refs.length !== manifest.originals.bytes || createHash("sha256").update(refs).digest("hex") !== manifest.originals.sha256) throw new Error("原文引用在校验后发生改变");
    referenceCheck = `OR (SELECT COALESCE(jsonb_agg(jsonb_build_object('key',original_object_key,'sourceType',source_type) ORDER BY original_object_key),'[]'::jsonb) FROM documents WHERE original_object_key IS NOT NULL) IS DISTINCT FROM convert_from(decode('${refs.toString("hex")}','hex'),'UTF8')::jsonb`;
  }
  if (manifest.externalOriginals && !target.externalOriginalsReady) throw new Error("备份引用外部原文，须先恢复对应 S3 桶并显式指定 --external-originals-ready true");
  await localDocker();
  const quarantine = await readFile(join(root, "infra/postgres/recovery-quarantine.sql"), "utf8");
  const receiver = pgProcess(target, "psql", ["-X", "-qAt", "--set", "ON_ERROR_STOP=1"]);
  const marker = name => new Promise((resolveMarker, reject) => {
    let output = "";
    const onData = chunk => {
      output += chunk;
      if (output.includes(name + "\n")) { receiver.child.stdout.removeListener("data", onData); resolveMarker(); }
      if (output.length > 4096) reject(new Error("恢复状态超限"));
    };
    receiver.child.stdout.on("data", onData);
    receiver.done.then(() => reject(new Error("恢复前置检查未确认")), reject);
  });
  // Stop other connections before importing anything. Rejection of a non-empty
  // database rolls back without changing its connection policy.
  const checked = marker("RECOVERY_CHECKED");
  receiver.child.stdin.write(`SET statement_timeout='10s'; SELECT pg_advisory_lock(704921342); ${emptyTarget} SELECT 'RECOVERY_CHECKED';\n`);
  const maintenance = { ...target, database: "postgres" };
  let connectionPolicyChanged = false;
  try {
    await checked;
    // PostgreSQL refuses disabling connections from the database itself.
    await pgQuery(maintenance, `ALTER DATABASE "${target.database}" ALLOW_CONNECTIONS false;`);
    connectionPolicyChanged = true;
    const frozen = marker("RECOVERY_FROZEN");
    receiver.child.stdin.write(`BEGIN; ${emptyTarget} SELECT 'RECOVERY_FROZEN'; SET statement_timeout=0;\n`);
    await frozen;
  } catch (error) {
    receiver.child.kill("SIGKILL"); await receiver.done.catch(() => {});
    if (connectionPolicyChanged) await pgQuery(maintenance, `ALTER DATABASE "${target.database}" ALLOW_CONNECTIONS true;`).catch(() => {});
    throw new Error("恢复前置检查失败：需要 PostgreSQL 16 空库，且无其他连接；目标数据未修改。" + (error.message.match(/SQLSTATE [0-9A-Z]{5}/)?.[0] ?? ""));
  }
  receiver.child.stdout.resume();
  const decoder = pgProcess(target, "pg_restore", ["--no-owner", "--no-acl", "--file=-"]);
  const digest = createHash("sha256"); let bytes = 0;
  const hashing = new Transform({ transform(chunk, encoding, callback) { digest.update(chunk); bytes += chunk.length; callback(null, chunk); } });
  const archive = pipeline(createReadStream(join(target.directory, "database.dump")), hashing, decoder.child.stdin);
  try {
    await Promise.all([archive, pipeline(decoder.child.stdout, receiver.child.stdin, { end: false }), decoder.done]);
    if (bytes !== manifest.dump.bytes || digest.digest("hex") !== manifest.dump.sha256) throw new Error("恢复读取期间备份发生改变，事务不提交");
    const restoredMetadata = `SET search_path=public,pg_catalog;
DO $$ BEGIN
 IF (SELECT jsonb_agg(jsonb_build_object('version',version,'checksum',encode(checksum,'hex')) ORDER BY version)
  FROM _sqlx_migrations WHERE success) IS DISTINCT FROM '${JSON.stringify(manifest.migrations)}'::jsonb
  OR EXISTS(SELECT 1 FROM _sqlx_migrations WHERE NOT success)
  OR (SELECT count(*) FROM documents WHERE original_object_key IS NOT NULL)<>${manifest.externalOriginals}
  ${referenceCheck}
 THEN RAISE EXCEPTION 'restored metadata mismatch'; END IF;
END $$;\n`;
    receiver.child.stdin.end(`\n${restoredMetadata}${quarantine}\nCOMMIT;\n`);
    await receiver.done;
    await pgQuery(maintenance, `ALTER DATABASE "${target.database}" ALLOW_CONNECTIONS true;`);
    return { restored: true, quarantined: true, policy: "offline-quarantine-v1", dumpSha256: manifest.dump.sha256 };
  } catch (error) {
    decoder.child.kill("SIGKILL"); receiver.child.kill("SIGKILL");
    await Promise.allSettled([archive, decoder.done, receiver.done]);
    throw new Error("恢复未确认成功；已进入恢复的空目标保持离线，勿启动服务。" + (error.message.startsWith("恢复读取") ? error.message : error.message.match(/SQLSTATE [0-9A-Z]{5}/)?.[0] ?? ""));
  }
}
