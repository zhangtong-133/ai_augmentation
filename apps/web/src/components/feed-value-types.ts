export const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
export const statuses: Record<string, string> = { draft: "待批准", authorized: "已批准，等待本机执行", running: "执行中", succeeded: "已完成", unknown: "执行结果未知", cancelled: "已取消", expired: "已到期", invalidated: "已失效" };
type Pricing = { kind: "local"; endpoint: string; model: string; valid_until_unix_ms: string } | { kind: "api"; available: false } | { kind: "subscription"; provider: string; model: string; configuration_version: string; connection_id: string; valid_until_unix_ms: string };
export type ValueSummary = { id: string; status: string; digest: string; pricing: Pricing; created_at_unix_ms: string; expires_at_unix_ms: string; approved_at_unix_ms: string | null; execution_mode: "local_only" };
export type ValueDetail = ValueSummary & { shared_content: { instructions: string; input: string } | null; candidates: { id: number; title: string; subscription_id: string; entry_key: string }[] | null; scores: { id: number; score: number | null; reason: string }[] | null };
export type Audit = { event: string; at_unix_ms: string };
function fail(): never { throw new Error("评分数据不完整，请核对原请求。"); }
export function timestamp(value: unknown): value is string { return typeof value === "string" && /^\d{1,16}$/.test(value) && Number(value) <= 8.64e15; }
export function localTarget(endpoint: unknown, model: unknown): boolean {
  if (typeof endpoint !== "string" || typeof model !== "string" || !/^[A-Za-z0-9._:/-]{1,128}$/.test(model) || model.endsWith(":cloud") || model.endsWith("-cloud")) return false;
  const address = /^http:\/\/(127\.(?:0|[1-9]\d{0,2})\.(?:0|[1-9]\d{0,2})\.(?:0|[1-9]\d{0,2})|\[::1\]):([1-9]\d{0,4})$/.exec(endpoint);
  return !!address && Number(address[2]) <= 65535 && (address[1] === "[::1]" || address[1].split(".").every(n => Number(n) <= 255));
}
export function summary(value: unknown): ValueSummary {
  const v = value as ValueSummary;
  if (!v || typeof v.id !== "string" || !uuid.test(v.id) || !Object.hasOwn(statuses, v.status) || typeof v.digest !== "string" || !/^[a-f0-9]{64}$/.test(v.digest)
    || v.execution_mode !== "local_only" || !timestamp(v.created_at_unix_ms) || !timestamp(v.expires_at_unix_ms) || !(v.approved_at_unix_ms === null || timestamp(v.approved_at_unix_ms))) fail();
  const p = v.pricing;
  if (!p || (p.kind !== "api" && p.kind !== "subscription" && p.kind !== "local")) fail();
  if (p.kind === "api") { if (p.available !== false) fail(); }
  else if (p.kind === "local") { if (!localTarget(p.endpoint, p.model) || !timestamp(p.valid_until_unix_ms)) fail(); }
  else if (p.provider !== "chatgpt-plan" || typeof p.model !== "string" || !p.model || typeof p.configuration_version !== "string" || !p.configuration_version || typeof p.connection_id !== "string" || !uuid.test(p.connection_id) || !timestamp(p.valid_until_unix_ms)) fail();
  return v;
}
export function detail(value: unknown, id: string): ValueDetail {
  const v = summary(value) as ValueDetail;
  if (v.id !== id) fail();
  if (v.shared_content !== null && (!v.shared_content || typeof v.shared_content.instructions !== "string" || typeof v.shared_content.input !== "string")) fail();
  if (v.candidates !== null && (!Array.isArray(v.candidates) || v.candidates.length > 100 || v.candidates.some((c, i) => !c || c.id !== i + 1 || typeof c.title !== "string" || typeof c.subscription_id !== "string" || typeof c.entry_key !== "string"))) fail();
  if ((v.shared_content === null) !== (v.candidates === null)) fail();
  if (v.scores !== null && (!Array.isArray(v.scores) || !v.candidates || v.scores.length !== v.candidates.length || new Set(v.scores.map(s => s?.id)).size !== v.scores.length || v.scores.some(s => !s || !v.candidates?.some(c => c.id === s.id) || !(s.score === null || (Number.isInteger(s.score) && s.score >= 0 && s.score <= 100)) || typeof s.reason !== "string" || !s.reason))) fail();
  if (["cancelled", "expired", "invalidated"].includes(v.status) && (v.shared_content !== null || v.scores !== null)) fail();
  if (v.status === "draft" && !v.shared_content) fail();
  return v;
}
export function page<T extends { id: string }>(value: unknown, parse: (value: unknown) => T, limit: number): { items: T[]; next_cursor: string | null } {
  const v = value as { items: unknown[]; next_cursor: string | null };
  if (!v || !Array.isArray(v.items) || v.items.length > limit || !(v.next_cursor === null || (typeof v.next_cursor === "string" && uuid.test(v.next_cursor)))) fail();
  const items = v.items.map(parse);
  if (new Set(items.map(item => item.id)).size !== items.length) fail();
  return { items, next_cursor: v.next_cursor };
}
export function audit(value: unknown): Audit[] {
  const v = value as { items: Audit[] };
  if (!v || !Array.isArray(v.items) || v.items.some(a => !a || typeof a.event !== "string" || !timestamp(a.at_unix_ms))) fail();
  return v.items;
}
export function date(value: string) { return new Date(Number(value)).toLocaleString(undefined, { timeZoneName: "short" }); }

export type ValueReadingItem = { id: number; title: string; summary: string; link: string | null; rule_score: number; model_score: number | null; reason: string };
export type ValueReading = { id: string; digest: string; status: "succeeded"; day_start_unix_ms: string; as_of_unix_ms: string; keywords: string[]; items: ValueReadingItem[] };
export function reading(value: unknown, expected: ValueDetail): ValueReading {
  const v = value as ValueReading;
  if (!v || v.id !== expected.id || v.digest !== expected.digest || v.status !== "succeeded" || !timestamp(v.day_start_unix_ms) || !timestamp(v.as_of_unix_ms)
    || !Array.isArray(v.keywords) || v.keywords.length < 1 || v.keywords.length > 5 || v.keywords.some(k => typeof k !== "string" || !k || [...k].length > 64)
    || !Array.isArray(v.items) || !expected.candidates || v.items.length !== expected.candidates.length || v.items.length < 1 || v.items.length > 20) fail();
  if (new Set(v.items.map(i => i?.id)).size !== v.items.length || v.items.some(i => !i || !Number.isInteger(i.id) || i.id < 1 || i.id > v.items.length
    || typeof i.title !== "string" || [...i.title].length > 512 || i.title !== expected.candidates?.[i.id - 1]?.title
    || typeof i.summary !== "string" || [...i.summary].length > 8192 || !(i.link === null || (typeof i.link === "string" && i.link.length <= 8192))
    || !Number.isInteger(i.rule_score) || i.rule_score < 0 || i.rule_score > 100
    || !(i.model_score === null || (Number.isInteger(i.model_score) && i.model_score >= 0 && i.model_score <= 100))
    || typeof i.reason !== "string" || !i.reason.trim() || [...i.reason].length > 240)) fail();
  return v;
}
