import { StatusHttpError, type LearningStatus, type StatusEvent } from "./learning-status-stream";
export type TextEvent = StatusEvent | { type: "delta"; text: string } | { type: "clear" };
const statuses: LearningStatus[] = ["draft", "authorized", "running", "succeeded", "unknown", "cancelled", "expired", "invalidated"];
const reasons = ["session_unavailable", "request_unavailable", "observation_unavailable", "observation_timeout"];
const encode = (text: string) => new TextEncoder().encode(text).length;
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("invalid event");
  return value as Record<string, unknown>;
}
function keys(value: Record<string, unknown>, expected: string[]) {
  if (Object.keys(value).sort().join() !== expected.sort().join()) throw new Error("invalid fields");
}
export class LearningTextParser {
  private buffer = ""; private sequence = 0; private bytes = 0; private textBytes = 0;
  private ended = false; private failed = false; private cleared = false; private started = false;
  constructor(private readonly request: string) {}
  push(text: string): TextEvent[] {
    if (this.failed) throw new Error("closed parser");
    try { return this.consume(text); } catch { this.failed = true; this.buffer = ""; throw new Error("invalid temporary text stream"); }
  }
  private consume(text: string): TextEvent[] {
    this.bytes += encode(text);
    if (this.bytes > 524288) throw new Error("stream too large");
    this.buffer += text; const result: TextEvent[] = [];
    let boundary: number;
    while ((boundary = this.buffer.indexOf("\n\n")) >= 0) {
      const frame = this.buffer.slice(0, boundary); this.buffer = this.buffer.slice(boundary + 2);
      if (encode(frame) > 4096 || this.ended || this.sequence >= 1100) throw new Error("invalid frame");
      const fields: Record<string, string> = {};
      for (const line of frame.split("\n")) {
        if (line.startsWith(":")) continue;
        const match = /^(event|id|data): (.*)$/.exec(line);
        if (!match || fields[match[1]] !== undefined) throw new Error("invalid frame");
        fields[match[1]] = match[2];
      }
      if (!Object.keys(fields).length) continue;
      keys(fields, ["event", "id", "data"]);
      if (fields.id !== String(this.sequence)) throw new Error("invalid sequence");
      const data = object(JSON.parse(fields.data)); keys(data, ["protocol_version", "request_id", "sequence", "detail"]);
      if (data.protocol_version !== "learning-text-v1" || data.request_id !== this.request || data.sequence !== fields.id) throw new Error("wrong request");
      const detail = object(data.detail);
      if (fields.event === "status") {
        keys(detail, ["status", "terminal"]);
        if (!statuses.includes(detail.status as LearningStatus) || typeof detail.terminal !== "boolean" || detail.terminal !== !["draft", "authorized", "running"].includes(String(detail.status))) throw new Error("invalid status");
        this.started = true; this.ended = detail.terminal;
        result.push({ type: "status", status: detail.status as LearningStatus, terminal: detail.terminal });
      } else if (fields.event === "closed") {
        keys(detail, ["reason"]);
        if (!reasons.includes(String(detail.reason))) throw new Error("invalid close");
        this.ended = true; result.push({ type: "closed", reason: detail.reason as Extract<StatusEvent, { type: "closed" }>["reason"] });
      } else if (fields.event === "delta") {
        keys(detail, ["text"]);
        if (!this.started || this.cleared || typeof detail.text !== "string" || !detail.text || encode(detail.text) > 512 || this.textBytes + encode(detail.text) > 24576) throw new Error("invalid text");
        this.textBytes += encode(detail.text); result.push({ type: "delta", text: detail.text });
      } else if (fields.event === "clear") {
        keys(detail, []); if (!this.started) throw new Error("missing status"); this.cleared = true; result.push({ type: "clear" });
      } else throw new Error("invalid event");
      this.sequence++;
    }
    if (encode(this.buffer) > 4096) throw new Error("frame too large");
    return result;
  }
  finish() { if (this.failed || !this.ended || this.buffer.length) throw new Error("incomplete stream"); }
}
export async function observeLearningText(request: string, signal: AbortSignal, receive: (event: TextEvent) => void) {
  const response = await fetch(`/api/learning/model-authorizations/${encodeURIComponent(request)}/text-events`, {
    cache: "no-store", redirect: "error", signal: AbortSignal.any([signal, AbortSignal.timeout(25000)]),
  });
  if (!response.ok) { await response.body?.cancel(); throw new StatusHttpError(response.status); }
  if (!response.body || response.headers.get("content-type")?.split(";")[0].trim() !== "text/event-stream") { await response.body?.cancel(); throw new Error("invalid content type"); }
  const reader = response.body.getReader(); const decoder = new TextDecoder("utf-8", { fatal: true });
  const parser = new LearningTextParser(request); let bytes = 0;
  try {
    for (;;) {
      if (signal.aborted) return;
      const { value, done } = await reader.read();
      if (signal.aborted) return;
      bytes += value?.byteLength ?? 0; if (bytes > 524288) throw new Error("stream too large");
      const events = parser.push(done ? decoder.decode() : decoder.decode(value, { stream: true }));
      for (const event of events) { if (signal.aborted) return; receive(event); }
      if (done) { parser.finish(); return; }
    }
  } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
}
