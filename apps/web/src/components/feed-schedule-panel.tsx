"use client";

import { useEffect, useRef, useState, type FormEvent } from "react";

type Subscription = { name: string; snapshot: { subscription_id: string; source_url: string; enabled: boolean } };
type Schedule = { plan: { subscription_revision: string; source_url: string; input: { schedule_id: string; starts_at_unix_ms: string; ends_at_unix_ms: string; interval_hours: number }; max_occurrences: number; approval_expires_at_unix_ms: string }; digest: string; status: string; approved_at_unix_ms: string | null };
type Audit = { event: string; at_unix_ms: string };
type Page<T> = { items: T[]; next_cursor: string | null };
type Operation = { path: string; id: string; kind: "preview" | "approve" | "cancel"; body: object };
const empty = <T,>(): Page<T> => ({ items: [], next_cursor: null });
const labels: Record<string, string> = { draft: "待同意", active: "已授权", cancelled: "已取消", expired: "已到期" };
const time = (value: string) => new Date(Number(value)).toLocaleString();
const suffix = (pages: string[]) => pages.length ? `?after=${encodeURIComponent(pages.at(-1)!)}` : "";

export function FeedSchedulePanel() {
  const [sources, setSources] = useState<Page<Subscription>>(empty);
  const [history, setHistory] = useState<Page<Schedule>>(empty);
  const [sourcePages, setSourcePages] = useState<string[]>([]);
  const [historyPages, setHistoryPages] = useState<string[]>([]);
  const [source, setSource] = useState("");
  const [start, setStart] = useState("");
  const [end, setEnd] = useState("");
  const [interval, setFrequency] = useState("24");
  const [selected, setSelected] = useState<Schedule | null>(null);
  const [audit, setAudit] = useState<Audit[]>([]);
  const [ack, setAck] = useState(false);
  const [pending, setPending] = useState<Operation | null>(null);
  const [ready, setReady] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [now, setNow] = useState(0);
  const active = useRef<AbortController | null>(null);
  const alive = useRef(true);

  async function request<T>(path: string, c: AbortController, operation?: Operation): Promise<T> {
    const response = await fetch(path, { method: operation ? "POST" : "GET", cache: "no-store", headers: operation ? { "Content-Type": "application/json", "X-Requested-With": "personal-ai" } : undefined, body: operation ? JSON.stringify(operation.body) : undefined, signal: AbortSignal.any([c.signal, AbortSignal.timeout(10000)]) });
    if (!response.ok) {
      if (response.status === 401 && alive.current && !c.signal.aborted) {
        c.abort(); setReady(false); setSources(empty()); setHistory(empty()); setSource(""); setStart(""); setEnd(""); setSelected(null); setAudit([]); setPending(null); setAck(false); setNotice("");
      }
      throw new Error(response.status === 401 ? "登录已失效，请重新登录。" : response.status === 404 ? "计划或订阅不存在，或无权访问。" : response.status === 409 ? "计划已失效、订阅已变化或额度已满，请核对原计划。" : [400, 422].includes(response.status) ? "请检查时间和频率：开始至少在五分钟后，结束不超过未来七天。" : "服务暂不可用，请核对原计划。");
    }
    return response.json();
  }
  async function run(work: (c: AbortController) => Promise<void>) {
    if (active.current) return;
    const c = new AbortController(); active.current = c; setBusy(true); setError(""); setAck(false);
    try { await work(c); } catch (e) {
      if (alive.current && active.current === c) setError(e instanceof Error && e.name === "Error" ? e.message : "连接中断，操作结果未确认，请核对原计划。");
    } finally { if (alive.current && active.current === c) { active.current = null; setBusy(false); } }
  }
  async function load(c: AbortController, sp: string[], hp: string[]) {
    const [s, h] = await Promise.all([request<Page<Subscription>>(`/api/feed-subscriptions${suffix(sp)}`, c), request<Page<Schedule>>(`/api/feed-schedules${suffix(hp)}`, c)]);
    if (!alive.current || c.signal.aborted) return;
    setSources(s); setHistory(h); setSourcePages(sp); setHistoryPages(hp); setSource(""); setReady(true);
  }
  useEffect(() => {
    alive.current = true;
    void run(c => load(c, [], []));
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => { alive.current = false; active.current?.abort(); active.current = null; clearInterval(timer); };
    // Account-keyed initial load; later reads are explicit and serialized.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  async function mutate(operation: Operation) {
    await run(async c => {
      setPending(operation); setNotice("");
      const saved = await request<Schedule>(operation.path, c, operation);
      if (!alive.current || c.signal.aborted) return;
      setPending(null); setSelected(saved); setAudit([]);
      setNotice(operation.kind === "preview" ? "预览已保存，尚未授权采集。" : "状态已保存；已授权不代表后台正在运行。");
      await load(c, sourcePages, []);
    });
  }
  function preview(event: FormEvent) {
    event.preventDefault(); if (pending || busy || !ready) return;
    const starts = new Date(start).getTime(), ends = new Date(end).getTime();
    if (!source || !Number.isSafeInteger(starts) || !Number.isSafeInteger(ends) || starts < Date.now() + 300000 || ends <= starts || ends > Date.now() + 7 * 86400000) { setError("开始至少在五分钟后，结束须晚于开始且不超过未来七天。"); return; }
    const id = crypto.randomUUID();
    void mutate({ id, kind: "preview", path: `/api/feed-subscriptions/${source}/schedules`, body: { schedule_id: id, starts_at_unix_ms: String(starts), ends_at_unix_ms: String(ends), interval_hours: Number(interval) } });
  }
  function inspect(id: string, recovery = false) {
    void run(async c => {
      const saved = await request<Schedule>(`/api/feed-schedules/${id}`, c);
      if (!alive.current || c.signal.aborted) return;
      setSelected(saved); setAudit([]);
      if (recovery) { setPending(null); setNotice("已读取原计划，请重新审阅当前状态。查询不会授权或采集。"); }
      const events = await request<Audit[]>(`/api/feed-schedules/${id}/audit`, c);
      if (alive.current && !c.signal.aborted) setAudit(events);
    });
  }
  function act(kind: "approve" | "cancel") {
    if (!selected || pending || busy || (kind === "approve" && (!ack || Date.now() >= Number(selected.plan.approval_expires_at_unix_ms)))) return;
    const id = selected.plan.input.schedule_id;
    void mutate({ id, kind, path: `/api/feed-schedules/${id}/${kind}`, body: kind === "approve" ? { accepted_digest: selected.digest, acknowledge_recurring_source_requests: true } : {} });
  }
  const locked = busy || !ready || pending !== null;
  const expired = selected ? now >= Number(selected.plan.approval_expires_at_unix_ms) : false;
  return <section className="feedPanel" aria-label="周期 RSS">
    <h2>周期 RSS</h2>
    <p>授权后按固定频率请求来源，到期自动停止。每次只读取 RSS，不抓取文章、图片或附件，不调用模型。</p>
    <p>后台默认关闭，需部署者独立启用；本页无法确认后台是否在线。已授权不代表正在采集，手动采集开关也不代表后台状态。</p>
    <form onSubmit={preview}>
      <h3>新建周期计划</h3>
      <label htmlFor="rss-schedule-source">周期订阅</label><select id="rss-schedule-source" required disabled={locked} value={source} onChange={e => { setSource(e.target.value); setAck(false); }}><option value="">请选择已启用的订阅</option>{sources.items.map(s => <option key={s.snapshot.subscription_id} value={s.snapshot.subscription_id} disabled={!s.snapshot.enabled}>{s.name}{s.snapshot.enabled ? "" : "（已停用）"}</option>)}</select>
      <p>新建或修改订阅后请刷新本页。修改、停用或删除订阅会取消其周期授权。</p>
      <div className="pagination"><button type="button" disabled={locked || !sourcePages.length} onClick={() => void run(c => load(c, sourcePages.slice(0, -1), historyPages))}>上一页周期订阅</button><button type="button" disabled={locked || !sources.next_cursor} onClick={() => void run(c => load(c, [...sourcePages, sources.next_cursor!], historyPages))}>下一页周期订阅</button></div>
      <label htmlFor="rss-schedule-start">开始时间（本地时区）</label><input id="rss-schedule-start" type="datetime-local" required disabled={locked} value={start} onChange={e => { setStart(e.target.value); setAck(false); }} />
      <label htmlFor="rss-schedule-end">结束时间（本地时区）</label><input id="rss-schedule-end" type="datetime-local" required disabled={locked} value={end} onChange={e => { setEnd(e.target.value); setAck(false); }} />
      <label htmlFor="rss-schedule-interval">采集间隔</label><select id="rss-schedule-interval" disabled={locked} value={interval} onChange={e => { setFrequency(e.target.value); setAck(false); }}><option value="1">每 1 小时</option><option value="6">每 6 小时</option><option value="24">每 24 小时</option></select>
      <p>开始至少在五分钟后，结束不超过未来七天。预览保存后须在五分钟内同意。</p><button disabled={locked || !source}>预览周期计划</button>
    </form>
    {error && <p role="alert">{error}</p>}{notice && <p role="status">{notice}</p>}
    <button disabled={busy} onClick={() => { setSelected(null); setAudit([]); void run(c => load(c, sourcePages, historyPages)); }}>刷新周期 RSS</button>
    {busy && <p role="status">正在处理周期计划…</p>}
    {pending && <div className="feedReview" aria-label="核对周期操作"><p>结果未确认，请核对原计划，避免创建替代计划。</p><code>{pending.id}</code><button disabled={busy} onClick={() => inspect(pending.id, true)}>核对原计划</button>{pending.kind !== "approve" && <button disabled={busy} onClick={() => void mutate(pending)}>重试原周期操作</button>}<button disabled={busy} onClick={() => { setPending(null); setSelected(null); setAudit([]); setAck(false); setNotice("已关闭提示，可从周期历史核对；此操作不取消授权。"); }}>关闭周期核对提示</button></div>}
    <h3>周期授权历史</h3>{ready && history.items.length === 0 && <p>暂无周期计划。</p>}
    <ul className="feedList">{history.items.map(s => <li key={s.plan.input.schedule_id}><p>{s.plan.source_url}</p><p>{labels[s.status] ?? "状态待核对"} · 每 {s.plan.input.interval_hours} 小时</p><button disabled={locked} onClick={() => inspect(s.plan.input.schedule_id)}>审阅周期计划</button></li>)}</ul>
    <div className="pagination"><button disabled={locked || !historyPages.length} onClick={() => void run(c => load(c, sourcePages, historyPages.slice(0, -1)))}>上一页周期计划</button><button disabled={locked || !history.next_cursor} onClick={() => void run(c => load(c, sourcePages, [...historyPages, history.next_cursor!]))}>下一页周期计划</button></div>
    {selected && <div className="feedReview" aria-label="周期 RSS 授权确认"><h3>{labels[selected.status] ?? "状态待核对"}</h3><p>来源：{selected.plan.source_url}</p><p>订阅版本：{selected.plan.subscription_revision}</p><p>计划：<code>{selected.plan.input.schedule_id}</code></p><p>开始：{time(selected.plan.input.starts_at_unix_ms)}</p><p>结束：{time(selected.plan.input.ends_at_unix_ms)}</p><p>每 {selected.plan.input.interval_hours} 小时 · 最多 {selected.plan.max_occurrences} 次</p><p>每次 GET 最多 1 MiB / 100 条 / 8 秒，不跟随重定向；每个时段仅在开始后的十分钟内派发，错过不补发，失败不自动重试。与手动采集共享每日额度。</p>
      {selected.status === "draft" && <><p>同意期限：{time(selected.plan.approval_expires_at_unix_ms)}</p>{expired && <p>预览已过期，请查询原计划并重新创建预览。</p>}<label><input type="checkbox" disabled={locked || expired} checked={ack} onChange={e => setAck(e.target.checked)} />我已核对来源、频率、期限和最多次数，同意周期请求该来源</label><button disabled={locked || expired || !ack} onClick={() => act("approve")}>同意周期采集</button></>}
      {selected.approved_at_unix_ms && <p>批准时间：{time(selected.approved_at_unix_ms)}</p>}
      {["draft", "active"].includes(selected.status) && <><p>取消会阻止后续采集，无法撤回已经发送的请求。</p><button disabled={locked} onClick={() => act("cancel")}>取消周期计划</button></>}
      <button disabled={locked} onClick={() => inspect(selected.plan.input.schedule_id)}>查询周期计划</button><h4>周期审计</h4><ul>{audit.map((a, i) => <li key={i}>{labels[a.event] ?? "状态变更"} · {time(a.at_unix_ms)}</li>)}</ul>
    </div>}
  </section>;
}
