export type Source = { document_id: string; ordinal: number; title: string; source: string; text: string };
export type Citation = Source & { id: number; score: number; quote: string; quote_start: number; quote_end: number };
type AnswerResult = { status: "answered"; answer: string; citations: Citation[] }
  | { status: "insufficient_evidence"; answer: null; citations: [] };
export type Execution = {
  status: "running" | "succeeded" | "failed" | "unknown" | "invalidated" | "cancelled";
  request_sha256: string; started_at_unix_ms: number; deadline_at_unix_ms: number;
  sent_at_unix_ms: number | null; finished_at_unix_ms: number | null;
  question: string | null; result: AnswerResult | null;
};
export type Grant = {
  request_id: string; status: string; digest: string; created_at_unix_ms: number; expires_at_unix_ms: number;
  execution_available: boolean; execution: Execution | null;
  preview: null | { question: string; materials: Source[]; request_sha256: string; request: { endpoint: string; body: unknown; profile: string } };
};
export const statuses: Record<string, string> = { draft: "待确认", authorized: "已保存授权", cancelled: "已取消", expired: "已过期", invalidated: "来源已失效", consumed: "已领取" };
export const executionStatuses: Record<Execution["status"], string> = {
  running: "正在执行一次问答", succeeded: "结果已保存，待核对", failed: "执行失败，不会重发",
  unknown: "结果未知，不会重发", invalidated: "来源已失效，结果已清除", cancelled: "已取消执行，结果已清除",
};
const sha = (value: unknown): value is string => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
const timestamp = (value: unknown): value is number => Number.isSafeInteger(value) && Number(value) >= 0;
const optionalTimestamp = (value: unknown): value is number | null => value === null || timestamp(value);
function isSource(value: unknown): value is Source {
  if (!value || typeof value !== "object") return false;
  const source = value as Source;
  return [source.document_id, source.title, source.source, source.text].every(item => typeof item === "string")
    && Number.isSafeInteger(source.ordinal) && source.ordinal >= 0;
}
function isCitation(value: unknown): value is Citation {
  if (!isSource(value)) return false;
  const citation = value as Citation;
  if (!Number.isInteger(citation.id) || citation.id < 1 || citation.id > 5 || !Number.isFinite(citation.score)
    || typeof citation.quote !== "string" || !citation.quote.trim() || Array.from(citation.quote).length > 400
    || !Number.isSafeInteger(citation.quote_start) || !Number.isSafeInteger(citation.quote_end)) return false;
  const text = Array.from(citation.text);
  if (citation.quote_start < 0 || citation.quote_end <= citation.quote_start || citation.quote_end > text.length
    || text.slice(citation.quote_start, citation.quote_end).join("") !== citation.quote) return false;
  const first = citation.text.indexOf(citation.quote);
  return first >= 0 && citation.text.indexOf(citation.quote, first + 1) === -1;
}
function isAnswerResult(value: unknown): value is AnswerResult {
  if (!value || typeof value !== "object") return false;
  const result = value as AnswerResult;
  if (!Array.isArray(result.citations)) return false;
  if (result.status === "insufficient_evidence") return result.answer === null && result.citations.length === 0;
  return result.status === "answered" && typeof result.answer === "string" && !!result.answer.trim()
    && Array.from(result.answer).length <= 4000 && result.citations.length > 0 && result.citations.length <= 5
    && result.citations.every(isCitation) && new Set(result.citations.map(citation => citation.id)).size === result.citations.length;
}
function isExecution(value: unknown, metadata: boolean): value is Execution {
  if (!value || typeof value !== "object") return false;
  const execution = value as Execution;
  return Object.hasOwn(executionStatuses, execution.status) && sha(execution.request_sha256)
    && timestamp(execution.started_at_unix_ms) && timestamp(execution.deadline_at_unix_ms)
    && execution.deadline_at_unix_ms >= execution.started_at_unix_ms
    && optionalTimestamp(execution.sent_at_unix_ms) && optionalTimestamp(execution.finished_at_unix_ms)
    && (execution.question === null || typeof execution.question === "string")
    && (metadata ? execution.question === null && execution.result === null
      : execution.status === "succeeded"
        ? typeof execution.question === "string" && isAnswerResult(execution.result)
        : execution.result === null && (execution.status === "running" || execution.question === null));
}
export function isGrant(value: unknown, metadata = false): value is Grant {
  if (!value || typeof value !== "object") return false;
  const grant = value as Grant;
  if (typeof grant.request_id !== "string" || !/^[0-9a-f-]{36}$/.test(grant.request_id) || !Object.hasOwn(statuses, grant.status)
    || !sha(grant.digest) || !timestamp(grant.created_at_unix_ms) || !timestamp(grant.expires_at_unix_ms)
    || typeof grant.execution_available !== "boolean"
    || !(grant.execution === null || grant.status === "consumed" && grant.preview === null && isExecution(grant.execution, metadata))) return false;
  if (grant.preview === null) return true;
  const preview = grant.preview;
  return !metadata && !!preview && ["draft", "authorized"].includes(grant.status)
    && typeof preview.question === "string" && sha(preview.request_sha256)
    && typeof preview.request?.endpoint === "string" && typeof preview.request.profile === "string"
    && !!preview.request.body && typeof preview.request.body === "object"
    && Array.isArray(preview.materials) && preview.materials.length > 0 && preview.materials.length <= 5 && preview.materials.every(isSource);
}
