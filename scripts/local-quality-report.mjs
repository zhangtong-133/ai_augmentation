import { open } from "node:fs/promises";
import { constants } from "node:fs";
import { createHash } from "node:crypto";
export async function readReport(path) {
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    if (!(await file.stat()).isFile()) throw new Error("not a report file");
    const buffer = Buffer.alloc(65537); let length = 0;
    while (length < buffer.length) {
      const { bytesRead } = await file.read(buffer, length, buffer.length - length, null);
      if (bytesRead === 0) break;
      length += bytesRead;
    }
    if (length > 65536) throw new Error("report too large");
    const bytes = buffer.subarray(0, length);
    const decoded = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return { report: JSON.parse(decoded), sha256: createHash("sha256").update(bytes).digest("hex") };
  } finally { await file.close(); }
}
