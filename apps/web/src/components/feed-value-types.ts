export const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
export const statuses: Record<string, string> = { draft: "待批准", authorized: "已批准，等待本机执行", running: "执行中", succeeded: "已完成", unknown: "执行结果未知", cancelled: "已取消", expired: "已到期", invalidated: "已失效" };
type Pricing = { kind: "api"; available: false } | { kind: "subscription"; provider: string; model: string; configuration_version: string; connection_id: string; valid_until_unix_ms: string };
export type ValueSummary = { id: string; status: string; digest: string; pricing: Pricing; created_at_unix_ms: string; expires_at_unix_ms: string; approved_at_unix_ms: string | null; execution_mode: "local_only" };
export type ValueDetail = ValueSummary & { shared_content: { instructions: string; input: string } | null; candidates: { id: number; title: string; subscription_id: string; entry_key: string }[] | null; scores: { id: number; score: number | null; reason: string }[] | null };
export type Audit = { event: string; at_unix_ms: string };
function fail(): never { throw new Error("评分数据不完整，请核对原请求。"); }
export function timestamp(value: unknown): value is string { return typeof value === "string" && /^\d{1,16}$/.test(value) && Number(value) <= 8.64e15; }
export function summary(value: unknown): ValueSummary {
  const v = value as ValueSummary;
  if (!v || typeof v.id !== "string" || !uuid.test(v.id) || !Object.hasOwn(statuses, v.status) || typeof v.digest !== "string" || !/^[a-f0-9]{64}$/.test(v.digest)
    || v.execution_mode !== "local_only" || !timestamp(v.created_at_unix_ms) || !timestamp(v.expires_at_unix_ms) || !(v.approved_at_unix_ms === null || timestamp(v.approved_at_unix_ms))) fail();
  const p = v.pricing;
  if (!p || (p.kind !== "api" && p.kind !== "subscription")) fail();
  if (p.kind === "api") { if (p.available !== false) fail(); }
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
