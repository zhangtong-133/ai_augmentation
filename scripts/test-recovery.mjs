import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { expectedMigrations, matchingMigrations, parseOptions, validateManifest, verify } from "./recovery-lib.mjs";

test("endpoint arguments reject ambiguous containers, databases, switches and duplicate paths", () => {
  const good = ["--container", "a".repeat(64), "--database", "personal_ai", "--directory", "/tmp/example"];
  assert.equal(parseOptions(good, "backup").database, "personal_ai");
  for (const args of [good.slice(0, -1), [...good, "--directory", "/tmp/second"], good.map(v => v === "personal_ai" ? "postgres" : v), good.map(v => v === "personal_ai" ? "x; SELECT 1" : v), good.map(v => v === "a".repeat(64) ? "postgres" : v), [...good, "--clean", "yes"]]) assert.throws(() => parseOptions(args, "backup"));
  assert.throws(() => parseOptions(good, "verify"));
  assert.equal(parseOptions([...good, "--external-originals-ready", "true"], "restore").externalOriginalsReady, true);
  assert.throws(() => parseOptions([...good, "--external-originals-ready", "false"], "restore"));
  assert.throws(() => parseOptions([...good, "--external-originals-ready", "true"], "backup"));
});
test("backup checksum, exact migrations and file names fail closed on corruption", async () => {
  const directory = await mkdtemp(join(tmpdir(), "recovery-contract-"));
  try {
    const bytes = Buffer.from("test archive");
    const manifest = { format: "personal-ai-postgres-backup-v1", createdAt: new Date().toISOString(), postgresMajor: 16,
      migrations: await expectedMigrations(), externalOriginals: 0,
      dump: { file: "database.dump", bytes: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") } };
    const save = value => writeFile(join(directory, "manifest.json"), JSON.stringify(value));
    await writeFile(join(directory, "database.dump"), bytes); await save(manifest);
    assert.deepEqual(await verify(directory), manifest);
    for (const invalid of [{ ...manifest, unknown: true }, { ...manifest, postgresMajor: 17 }, { ...manifest, dump: { ...manifest.dump, file: "../database.dump" } }]) assert.throws(() => validateManifest(invalid));
    assert.equal(matchingMigrations([...manifest.migrations].reverse(), manifest.migrations), false);
    await save({ ...manifest, migrations: manifest.migrations.slice(1) }); await assert.rejects(verify(directory), /迁移/);
    await save(manifest); await writeFile(join(directory, "database.dump"), "corrupted"); await assert.rejects(verify(directory), /SHA-256/);
    await rm(join(directory, "database.dump")); await symlink("manifest.json", join(directory, "database.dump")); await assert.rejects(verify(directory), /符号链接/);
  } finally { await rm(directory, { recursive: true, force: true }); }
});
