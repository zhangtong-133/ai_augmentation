// This client understands status snapshots only, never provider text or executable actions.
const statuses = ["draft", "authorized", "running", "succeeded", "unknown", "cancelled", "expired", "invalidated"] as const;
export type LearningStatus = typeof statuses[number];
type Reason = "session_unavailable" | "request_unavailable" | "observation_unavailable" | "observation_timeout";
export type StatusEvent = { type: "status"; status: LearningStatus; terminal: boolean } | { type: "closed"; reason: Reason };
const reasons: Reason[] = ["session_unavailable", "request_unavailable", "observation_unavailable", "observation_timeout"];
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("invalid event");
  return value as Record<string, unknown>;
}
function keys(value: Record<string, unknown>, expected: string[]) {
  if (Object.keys(value).sort().join() !== expected.sort().join()) throw new Error("invalid fields");
}
export class LearningStatusParser {
  private buffer = "";
  private sequence = 0;
  private ended = false;
  private bytes = 0;
  constructor(private readonly request: string) {}
  push(text: string): StatusEvent[] {
    this.bytes += new TextEncoder().encode(text).length;
    if (this.bytes > 32768) throw new Error("stream too large");
    this.buffer += text;
    const result: StatusEvent[] = [];
    let boundary: number;
    while ((boundary = this.buffer.indexOf("\n\n")) >= 0) {
      const frame = this.buffer.slice(0, boundary); this.buffer = this.buffer.slice(boundary + 2);
      if (frame.length > 2048 || this.ended) throw new Error("invalid frame");
      const fields: Record<string, string> = {};
      for (const line of frame.split("\n")) {
        if (line.startsWith(":")) continue;
        const match = /^(event|id|data): (.*)$/.exec(line);
        if (!match || fields[match[1]] !== undefined) throw new Error("invalid frame");
        fields[match[1]] = match[2];
      }
      if (!Object.keys(fields).length) continue;
      if (fields.id !== String(this.sequence) || !["status", "closed"].includes(fields.event)) throw new Error("invalid sequence");
      const data = object(JSON.parse(fields.data));
      keys(data, ["protocol_version", "request_id", "sequence", "detail"]);
      if (data.protocol_version !== "learning-status-v1" || data.request_id !== this.request || data.sequence !== fields.id) throw new Error("wrong request");
      const detail = object(data.detail);
      if (fields.event === "status") {
        keys(detail, ["status", "terminal"]);
        if (!statuses.includes(detail.status as LearningStatus) || typeof detail.terminal !== "boolean" || detail.terminal !== !["draft", "authorized", "running"].includes(String(detail.status))) throw new Error("invalid status");
        result.push({ type: "status", status: detail.status as LearningStatus, terminal: detail.terminal });
        this.ended = detail.terminal;
      } else {
        keys(detail, ["reason"]);
        if (!reasons.includes(detail.reason as Reason)) throw new Error("invalid close");
        result.push({ type: "closed", reason: detail.reason as Reason }); this.ended = true;
      }
      this.sequence++;
    }
    if (this.buffer.length > 2048) throw new Error("frame too large");
    return result;
  }
  finish() { if (!this.ended || this.buffer.length) throw new Error("incomplete stream"); }
}
export class StatusHttpError extends Error {
  constructor(readonly status: number) { super("status unavailable"); }
}
export async function observeLearningStatus(request: string, signal: AbortSignal, receive: (event: StatusEvent) => void) {
  const response = await fetch(`/api/learning/model-authorizations/${encodeURIComponent(request)}/events`, {
    cache: "no-store", redirect: "error", signal: AbortSignal.any([signal, AbortSignal.timeout(25000)]),
  });
  if (!response.ok) { await response.body?.cancel(); throw new StatusHttpError(response.status); }
  if (!response.body || response.headers.get("content-type")?.split(";")[0].trim() !== "text/event-stream") { await response.body?.cancel(); throw new Error("invalid content type"); }
  const reader = response.body.getReader(); const decoder = new TextDecoder("utf-8", { fatal: true });
  const parser = new LearningStatusParser(request); let bytes = 0;
  try {
    for (;;) {
      if (signal.aborted) return;
      const { value, done } = await reader.read();
      if (signal.aborted) return;
      bytes += value?.byteLength ?? 0;
      if (bytes > 32768) throw new Error("stream too large");
      const events = parser.push(done ? decoder.decode() : decoder.decode(value, { stream: true }));
      for (const event of events) { if (signal.aborted) return; receive(event); }
      if (done) { parser.finish(); return; }
    }
  } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
}
