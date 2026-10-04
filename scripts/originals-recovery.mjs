import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { lstat, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { backup, digestFile, parseOptions, root, verify } from "./recovery-lib.mjs";

const keyPattern = /^users\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\/documents\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/;
const limits = { markdown: 256 * 1024, pdf: 5 * 1024 * 1024, web_page: 1024 * 1024 };
const nameFor = key => createHash("sha256").update(key).digest("hex") + ".bin";
const same = (a, b) => a.bytes === b.bytes && a.sha256 === b.sha256;
async function regular(path, limit) {
  const info = await lstat(path);
  if (!info.isFile() || info.size > limit) throw new Error("原文备份需要有界普通文件，拒绝符号链接");
  return info;
}
function entries(value) {
  if (!Array.isArray(value) || value.length > 1000) throw new Error("原文引用最多 1000 项");
  let previous = "";
  for (const entry of value) {
    if (!entry || Object.keys(entry).sort().join(",") !== "key,sourceType" || !keyPattern.test(entry.key) || !Object.hasOwn(limits, entry.sourceType) || entry.key <= previous) throw new Error("原文引用格式、排序或归属路径不合法");
    previous = entry.key;
  }
  return value;
}
async function references(directory, manifest) {
  if (!manifest.originals) throw new Error("旧备份没有快照原文引用，不能自动恢复外部原文");
  const path = join(directory, "originals.json");
  await regular(path, 1048576);
  if (!same(await digestFile(path), manifest.originals)) throw new Error("原文引用完整性校验失败");
  const refs = entries(JSON.parse(await readFile(path, "utf8")));
  if (refs.length !== manifest.externalOriginals) throw new Error("原文引用数量不一致");
  return refs;
}
// Only this explicit adapter config enters the child. No database URL/cloud defaults.
export function objectOperation(mode, key, bytes, env = process.env) {
  return new Promise((resolveOperation, reject) => {
    const config = { PATH: env.PATH };
    for (const name of ["OBJECT_STORE_ENABLED", "OBJECT_STORE_ENDPOINT", "OBJECT_STORE_BUCKET", "OBJECT_STORE_REGION", "OBJECT_STORE_ACCESS_KEY", "OBJECT_STORE_SECRET_KEY"]) if (env[name]) config[name] = env[name];
    const child = spawn(join(root, "target/debug/original-archive"), [mode, key], { cwd: root, env: config, stdio: ["pipe", "pipe", "pipe"] });
    const chunks = []; let size = 0;
    const timer = setTimeout(() => child.kill("SIGKILL"), 75000);
    child.stdin.on("error", () => {}); child.stderr.resume();
    child.stdout.on("data", chunk => { size += chunk.length; if (size > 5 * 1024 * 1024) child.kill("SIGKILL"); else chunks.push(chunk); });
    child.on("error", () => { clearTimeout(timer); reject(new Error("原文归档桥接无法启动，请先构建 original-archive")); });
    child.on("close", code => { clearTimeout(timer); code === 0 && size <= 5 * 1024 * 1024 ? resolveOperation(Buffer.concat(chunks)) : reject(new Error("原文存储操作失败，未输出凭据或原文")); });
    child.stdin.end(bytes);
  });
}
export async function exportOriginals(directory, manifest, env = process.env) {
  const refs = await references(directory, manifest), destination = join(directory, "objects");
  await mkdir(destination, { mode: 0o700 });
  try {
    const objects = [];
    for (const ref of refs) {
      const bytes = await objectOperation("read", ref.key, undefined, env);
      if (!bytes.length || bytes.length > limits[ref.sourceType]) throw new Error("原文超出对应格式的大小范围");
      const file = nameFor(ref.key);
      await writeFile(join(destination, file), bytes, { flag: "wx", mode: 0o600 });
      objects.push({ ...ref, file, bytes: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") });
    }
    const index = { format: "personal-ai-originals-backup-v1", databaseDumpSha256: manifest.dump.sha256, referencesSha256: manifest.originals.sha256, objects };
    await writeFile(join(destination, "index.json"), JSON.stringify(index) + "\n", { flag: "wx", mode: 0o600 });
    await verifyOriginals(directory, manifest);
    return { verified: true, originals: objects.length };
  } catch (error) { await rm(destination, { recursive: true, force: true }); throw error; }
}
export async function verifyOriginals(directory, suppliedManifest) {
  const manifest = suppliedManifest ?? await verify(directory), refs = await references(directory, manifest);
  const destination = join(directory, "objects");
  if (!(await lstat(destination)).isDirectory()) throw new Error("原文目录必须是真实目录，拒绝符号链接");
  const indexPath = join(destination, "index.json"); await regular(indexPath, 1048576);
  const index = JSON.parse(await readFile(indexPath, "utf8"));
  if (!index || Object.keys(index).sort().join(",") !== "databaseDumpSha256,format,objects,referencesSha256" || index.format !== "personal-ai-originals-backup-v1" || index.databaseDumpSha256 !== manifest.dump.sha256 || index.referencesSha256 !== manifest.originals.sha256 || !Array.isArray(index.objects) || index.objects.length !== refs.length) throw new Error("原文归档与数据库备份不匹配");
  for (let i = 0; i < refs.length; i++) {
    const item = index.objects[i], ref = refs[i];
    if (!item || Object.keys(item).sort().join(",") !== "bytes,file,key,sha256,sourceType" || item.key !== ref.key || item.sourceType !== ref.sourceType || item.file !== nameFor(ref.key) || !Number.isSafeInteger(item.bytes) || item.bytes <= 0 || item.bytes > limits[ref.sourceType] || !/^[a-f0-9]{64}$/.test(item.sha256)) throw new Error("原文文件清单不合法");
    const path = join(destination, item.file); await regular(path, limits[ref.sourceType]);
    if (!same(await digestFile(path), item)) throw new Error("原文文件大小或 SHA-256 不匹配");
  }
  return index;
}
export async function checkOriginals(directory, env = process.env) {
  const index = await verifyOriginals(directory);
  for (const item of index.objects) {
    const current = JSON.parse(await objectOperation("check", item.key, undefined, env));
    if (!current.exists || !same(current, item)) throw new Error("目标桶原文缺失或字节校验不匹配，拒绝开放数据库恢复");
  }
  return { verified: true, originals: index.objects.length };
}
export async function restoreOriginals(directory, env = process.env) {
  // Verify every local file before the first write. Never overwrite/delete a key.
  const index = await verifyOriginals(directory);
  for (const item of index.objects) {
    const current = JSON.parse(await objectOperation("check", item.key, undefined, env));
    if (current.exists) {
      if (!same(current, item)) throw new Error("目标桶存在不同内容，拒绝覆盖；已写入的对象保留供核对");
    } else {
      const bytes = await readFile(join(directory, "objects", item.file));
      if (!same({ bytes: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") }, item)) throw new Error("原文在校验后发生变化");
      await objectOperation("create", item.key, bytes, env);
    }
  }
  return checkOriginals(directory, env);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const [command, ...args] = process.argv.slice(2);
    if (command === "backup") {
      const manifest = await backup(parseOptions(args, "backup"), { captureOriginals: exportOriginals });
      console.log(JSON.stringify({ verified: true, databaseAndOriginals: true, originals: manifest.externalOriginals }));
    } else if (["verify", "restore", "check"].includes(command)) {
      const { directory } = parseOptions(args, "verify");
      const result = command === "verify" ? { verified: true, originals: (await verifyOriginals(directory)).objects.length } : await (command === "restore" ? restoreOriginals : checkOriginals)(directory);
      console.log(JSON.stringify(result));
    } else throw new Error("用法：originals-recovery.mjs backup --container 完整ID --database 名称 --directory 新目录 | verify|restore|check --directory 目录（显式 OBJECT_STORE_* 环境配置）");
  } catch { console.error("原文备份恢复失败；请核对用法、完整性、显式桶配置及目标冲突；未输出路径、凭据或原文"); process.exitCode = 1; }
}
