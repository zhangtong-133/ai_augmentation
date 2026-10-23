import { isGrant, type Grant } from "../components/answer-authorization-types";

export class AnswerExecutionHttpError extends Error {
  constructor(readonly status: number) { super("execution status unavailable"); }
}

function wait(signal: AbortSignal) {
  return new Promise<void>((resolve, reject) => {
    signal.throwIfAborted();
    const cancel = () => { clearTimeout(timer); reject(signal.reason); };
    const timer = setTimeout(() => { signal.removeEventListener("abort", cancel); resolve(); }, 2000);
    signal.addEventListener("abort", cancel, { once: true });
  });
}

// Reads the existing durable detail only. A stopped or interrupted observation
// never creates, approves, cancels or executes an authorization.
export async function observeAnswerExecution(initial: Grant, signal: AbortSignal, receive: (grant: Grant) => void) {
  const execution = initial.execution;
  if (!execution || execution.status !== "running") throw new Error("execution is not running");
  const limit = new AbortController();
  const timer = setTimeout(() => limit.abort(), 120000);
  const watching = AbortSignal.any([signal, limit.signal]);
  try {
    for (;;) {
      await wait(watching);
      watching.throwIfAborted();
      const response = await fetch(`/api/knowledge/answer-authorizations/${encodeURIComponent(initial.request_id)}`, {
        method: "GET", cache: "no-store", redirect: "error",
        signal: AbortSignal.any([watching, AbortSignal.timeout(20000)]),
      });
      if (!response.ok) {
        await response.body?.cancel();
        throw new AnswerExecutionHttpError(response.status);
      }
      const data: unknown = await response.json();
      watching.throwIfAborted();
      if (!isGrant(data) || data.request_id !== initial.request_id || data.digest !== initial.digest
        || !data.execution || data.execution.request_sha256 !== execution.request_sha256
        || data.execution.started_at_unix_ms !== execution.started_at_unix_ms
        || data.execution.deadline_at_unix_ms !== execution.deadline_at_unix_ms) {
        throw new Error("invalid execution status");
      }
      receive(data);
      if (data.execution.status !== "running") return "terminal";
    }
  } catch (error) {
    if (signal.aborted) return "stopped";
    if (limit.signal.aborted) return "timeout";
    throw error;
  } finally { clearTimeout(timer); }
}
