import { spawn } from "node:child_process";

const STAGES = ["select", "verify"];
const failure = () => Object.assign(new Error("native stage handshake unconfirmed"), { answerFailure: "protocol", answerFailureStage: "native_guard" });

// No inference acknowledgment is sent until the host confirms that particular cold request.
export function stagedCli(binary, args, options, before, signal, timeoutMs = 90000) {
  return new Promise((accept, reject) => {
    const controller = new AbortController();
    const child = spawn(binary, args, { ...options, stdio: ["pipe", "pipe", "pipe"] });
    let error, stdout = "", stderr = "", line = "", bytes = 0, seen = 0, allowed = 0, pending, forceTimer, closed = false;
    const stop = reason => {
      error ??= reason;
      controller.abort();
      child.kill("SIGTERM");
      if (!closed) forceTimer ??= setTimeout(() => child.kill("SIGKILL"), 1000);
    };
    const cancelled = () => stop(new Error("native request cancelled"));
    signal?.addEventListener("abort", cancelled, { once: true });
    if (signal?.aborted) cancelled();
    const timer = setTimeout(() => stop(new Error("native request timed out")), timeoutMs);
    child.stdin.on("error", () => stop(new Error("native permission channel unavailable")));
    child.on("error", stop);
    child.stdout.setEncoding("utf8"); child.stderr.setEncoding("utf8");
    const bounded = data => {
      bytes += Buffer.byteLength(data);
      if (bytes > 65536) { stop(failure()); return false; }
      return !error;
    };
    child.stdout.on("data", data => { if (bounded(data)) stdout += data; });
    child.stderr.on("data", data => {
      if (!bounded(data)) return;
      stderr += data; line += data;
      while (line.includes("\n")) {
        const end = line.indexOf("\n"), record = line.slice(0, end); line = line.slice(end + 1);
        if (!record.startsWith("ANSWER_NATIVE_STAGE=")) continue;
        const stage = record.slice("ANSWER_NATIVE_STAGE=".length);
        if (pending || stage !== STAGES[seen]) { stop(failure()); return; }
        seen++;
        pending = (async () => {
          await before(stage, controller.signal);
          if (error || closed || controller.signal.aborted) throw new Error("native permission cancelled");
          await new Promise((resolve, fail) => child.stdin.write("allow\n", issue => issue ? fail(issue) : resolve()));
          allowed++;
        })().catch(stop).finally(() => { pending = null; });
      }
    });
    child.on("close", code => {
      closed = true;
      // A stalled host check must not keep a cancelled child alive or grant a late permission.
      if (pending) controller.abort();
      clearTimeout(timer); clearTimeout(forceTimer);
      signal?.removeEventListener("abort", cancelled);
      if (error) { reject(error); return; }
      if (code !== 0) { reject(Object.assign(new Error("native case unconfirmed"), { stderr })); return; }
      if (seen !== 2 || allowed !== 2 || line) { reject(failure()); return; }
      try { accept(JSON.parse(stdout)); } catch { reject(failure()); }
    });
  });
}
