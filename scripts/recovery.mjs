import { backup, parseOptions, verify } from "./recovery-lib.mjs";
import { restore } from "./recovery-restore.mjs";
import { checkOriginals } from "./originals-recovery.mjs";

try {
  const [command, ...args] = process.argv.slice(2);
  if (command === "--help" && !args.length) {
    console.log("recovery.mjs backup --container 完整容器_ID --database 数据库名 --directory 新备份目录\nrecovery.mjs verify --directory 备份目录\nrecovery.mjs restore --container 完整容器_ID --database 空数据库名 --directory 备份目录 [--external-originals-ready true]\nrecovery.mjs restore-with-originals --container 完整容器_ID --database 空数据库名 --directory 备份目录（先字节核对显式配置的 S3 桶）");
  } else if (["backup", "verify", "restore", "restore-with-originals"].includes(command)) {
    const options = parseOptions(args, command === "restore-with-originals" ? "backup" : command);
    if (command === "restore-with-originals") {
      await checkOriginals(options.directory);
      console.log(JSON.stringify(await restore({ ...options, externalOriginalsReady: true })));
    } else if (command === "restore") { console.log(JSON.stringify(await restore(options))); }
    else {
      const manifest = command === "backup" ? await backup(options) : await verify(options.directory);
      console.log(JSON.stringify({ command, verified: true, postgresMajor: manifest.postgresMajor,
        migrations: manifest.migrations.length, externalOriginals: manifest.externalOriginals, dumpBytes: manifest.dump.bytes, dumpSha256: manifest.dump.sha256 }));
    }
  } else throw new Error("用法：node scripts/recovery.mjs --help");
} catch (error) {
  // File-system errors may contain paths; avoid dumping exception objects/stacks.
  console.error(error.code ? "备份文件读写失败，请核对目录及权限" : error.message);
  process.exitCode = 1;
}
