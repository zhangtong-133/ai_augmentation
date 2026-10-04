import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { expectedMigrations, matchingMigrations, parseOptions, validateManifest, verify } from "./recovery-lib.mjs";
import { verifyOriginals } from "./originals-recovery.mjs";

test("endpoint arguments reject ambiguous containers, databases, switches and duplicate paths", () => {
  const good = ["--container", "a".repeat(64), "--database", "personal_ai", "--directory", "/tmp/example"];
  assert.equal(parseOptions(good, "backup").database, "personal_ai");
  for (const args of [good.slice(0, -1), [...good, "--directory", "/tmp/second"], good.map(v => v === "personal_ai" ? "postgres" : v), good.map(v => v === "personal_ai" ? "x; SELECT 1" : v), good.map(v => v === "a".repeat(64) ? "postgres" : v), [...good, "--clean", "yes"]]) assert.throws(() => parseOptions(args, "backup"));
  assert.throws(() => parseOptions(good, "verify"));
  assert.equal(parseOptions([...good, "--external-originals-ready", "true"], "restore").externalOriginalsReady, true);
  assert.throws(() => parseOptions([...good, "--external-originals-ready", "false"], "restore"));
  assert.throws(() => parseOptions([...good, "--external-originals-ready", "true"], "backup"));
});

test("original archive binds to the database snapshot and rejects missing, corrupt, escaped and symlinked files", async () => {
  const directory = await mkdtemp(join(tmpdir(), "original-archive-contract-"));
  const hash = bytes => createHash("sha256").update(bytes).digest("hex");
  try {
    const id = "00000000-0000-4000-8000-000000000001", key = `users/${id}/documents/${id}/${id}`;
    const bytes = Buffer.from("原文 Unicode"), dump = Buffer.from("offline validation fixture");
    const refs = Buffer.from(JSON.stringify([{ key, sourceType: "markdown" }]));
    const file = hash(key) + ".bin";
    const manifest = { format: "personal-ai-postgres-backup-v1", createdAt: new Date().toISOString(), postgresMajor: 16, migrations: await expectedMigrations(), externalOriginals: 1,
      dump: { file: "database.dump", bytes: dump.length, sha256: hash(dump) }, originals: { file: "originals.json", bytes: refs.length, sha256: hash(refs) } };
    const index = { format: "personal-ai-originals-backup-v1", databaseDumpSha256: hash(dump), referencesSha256: hash(refs), objects: [{ key, sourceType: "markdown", file, bytes: bytes.length, sha256: hash(bytes) }] };
    const save = value => writeFile(join(directory, "objects/index.json"), JSON.stringify(value));
    await mkdir(join(directory, "objects"));
    await writeFile(join(directory, "manifest.json"), JSON.stringify(manifest));
    await writeFile(join(directory, "database.dump"), dump); await writeFile(join(directory, "originals.json"), refs);
    await writeFile(join(directory, "objects", file), bytes); await save(index);
    assert.equal((await verifyOriginals(directory)).objects.length, 1);
    for (const invalid of [{ ...index, databaseDumpSha256: "f".repeat(64) }, { ...index, objects: [] }, { ...index, objects: [{ ...index.objects[0], file: "../database.dump" }] }]) {
      await save(invalid); await assert.rejects(verifyOriginals(directory));
    }
    await save(index); await writeFile(join(directory, "objects", file), "changed"); await assert.rejects(verifyOriginals(directory), /SHA-256/);
    await rm(join(directory, "objects", file)); await symlink("../originals.json", join(directory, "objects", file)); await assert.rejects(verifyOriginals(directory), /符号链接/);
    await rm(join(directory, "objects"), { recursive: true }); await mkdir(join(directory, "alternate")); await symlink("alternate", join(directory, "objects")); await assert.rejects(verifyOriginals(directory), /符号链接/);
  } finally { await rm(directory, { recursive: true, force: true }); }
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
