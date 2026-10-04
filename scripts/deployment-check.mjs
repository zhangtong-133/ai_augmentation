import { readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { expectedMigrations, localDocker, matchingMigrations, parseOptions, pgQuery, root } from "./recovery-lib.mjs";
import { checkOriginals, verifyOriginals } from "./originals-recovery.mjs";

export function deploymentOptions(args) {
  const values = {};
  for (let i = 0; i < args.length; i += 2) {
    const key = args[i];
    if (!["--container", "--database", "--profile", "--originals"].includes(key) || !args[i + 1] || Object.hasOwn(values, key)) throw new Error("部署诊断参数不支持、缺失或重复");
    values[key] = args[i + 1];
  }
  if (!["current", "recovery"].includes(values["--profile"])) throw new Error("需要显式 --profile current 或 recovery");
  const target = parseOptions(["--container", values["--container"], "--database", values["--database"], "--directory", root], "backup");
  return { container: target.container, database: target.database, profile: values["--profile"], originals: values["--originals"] ? resolve(values["--originals"]) : undefined };
}
export async function checkDeployment(options, env = process.env) {
  const target = deploymentOptions(["--container", options.container, "--database", options.database, "--profile", options.profile, ...(options.originals ? ["--originals", options.originals] : [])]);
  await localDocker();
  const state = JSON.parse(await pgQuery(target, await readFile(join(root, "infra/postgres/deployment-check.sql"), "utf8")));
  const issues = [];
  const migrationsMatch = matchingMigrations(state.migrations, await expectedMigrations());
  if (state.postgresMajor !== 16) issues.push("postgres_version_mismatch");
  if (!migrationsMatch || state.failedMigrations !== 0) issues.push("migration_mismatch");
  let originalsVerified = state.externalOriginals === 0;
  if (state.externalOriginals > 0 && target.originals) {
    try {
      const index = await verifyOriginals(target.originals);
      if (!Array.isArray(state.originalRefs) || index.objects.length !== state.originalRefs.length || !index.objects.every((entry, i) => entry.key === state.originalRefs[i].key && entry.sourceType === state.originalRefs[i].sourceType)) throw new Error("reference mismatch");
      await checkOriginals(target.originals, env);
      originalsVerified = true;
    } catch { issues.push("originals_mismatch_or_unavailable"); }
  } else if (!originalsVerified) issues.push("originals_not_verified");
  if (target.profile === "recovery") {
    for (const key of ["sessions", "activeMcp", "activeConfigurations", "unsettledMoney", "activeJobs"]) if (state[key] !== 0) issues.push(`recovery_${key}_remain`);
  }
  return { ready: issues.length === 0, profile: target.profile, postgresMajor: state.postgresMajor, migrationsMatch,
    counts: Object.fromEntries(["sessions", "activeMcp", "activeConfigurations", "unsettledMoney", "activeJobs", "unknownModelRequests", "externalOriginals"].map(key => [key, state[key]])),
    originalsVerified, issues };
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv.length === 3 && process.argv[2] === "--help") {
      console.log("deployment-check.mjs --container 完整容器ID --database 数据库名 --profile current|recovery [--originals 原文备份目录]\n仅检查 PostgreSQL 迁移、执行状态和指定归档对应的原文，不启动任务或修改数据。");
    } else {
      const report = await checkDeployment(deploymentOptions(process.argv.slice(2)));
      console.log(JSON.stringify(report));
      if (!report.ready) process.exitCode = 1;
    }
  } catch { console.error("部署只读诊断未完成，请核对显式容器、数据库、profile、当前迁移及工具；未输出地址、凭据或私有数据"); process.exitCode = 1; }
}
