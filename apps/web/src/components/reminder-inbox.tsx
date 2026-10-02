"use client";

import { useEffect, useRef, useState } from "react";

type Reminder = { request_id: string; title: string; body: string; delivered_at_unix_ms: string; revision: string; read_at_unix_ms: string | null; archived_at_unix_ms: string | null };
type Page = { items: Reminder[]; next_cursor: string | null };
const empty = (): Page => ({ items: [], next_cursor: null });

export function ReminderInbox({ revision, onExpired }: { revision: number; onExpired: () => void }) {
  const [data, setData] = useState<Page>(empty);
  const [archived, setArchived] = useState(false);
  const [pages, setPages] = useState<string[]>([]);
  const [refresh, setRefresh] = useState(0);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const [error, setError] = useState("");
  const active = useRef<AbortController | null>(null);
  const readRequest = useRef<AbortController | null>(null);
  const cursor = pages.at(-1);
  useEffect(() => () => { active.current?.abort(); active.current = null; }, []);
  useEffect(() => {
    const c = new AbortController(); readRequest.current = c;
    async function load() {
      setLoading(true);
      try {
        const query = new URLSearchParams({ archived: String(archived) });
        if (cursor) query.set("after", cursor);
        const response = await fetch(`/api/reminders?${query}`, { cache: "no-store", signal: AbortSignal.any([c.signal, AbortSignal.timeout(10000)]) });
        if (c.signal.aborted) return;
        if (response.status === 401) { setData(empty()); onExpired(); return; }
        if (!response.ok) throw new Error("提醒服务暂不可用，请刷新重试。");
        const result: Page = await response.json();
        if (!c.signal.aborted) { setData(result); setUncertain(false); setError(""); }
      } catch (e) {
        if (!c.signal.aborted) { setData(empty()); setError(e instanceof Error && e.name === "Error" ? e.message : "无法读取提醒，请刷新重试。"); }
      } finally { if (!c.signal.aborted) setLoading(false); }
    }
    if (!active.current) void load();
    return () => c.abort();
  }, [revision, refresh, cursor, archived, onExpired]);

  async function update(item: Reminder, read: boolean, archive: boolean) {
    if (active.current || loading || uncertain) return;
    readRequest.current?.abort();
    const c = new AbortController(); active.current = c; setBusy(true); setError("");
    try {
      const response = await fetch(`/api/reminders/${item.request_id}`, { method: "PUT", headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" }, body: JSON.stringify({ revision: item.revision, read, archived: archive }), signal: AbortSignal.any([c.signal, AbortSignal.timeout(10000)]) });
      if (active.current !== c) return;
      if (response.status === 401) { setData(empty()); onExpired(); return; }
      if (!response.ok) throw new Error(response.status === 409 ? "提醒已在其他页面更新，请刷新后核对。" : "无法确认提醒更新结果，请刷新收件箱核对。");
      await response.json();
      if (active.current !== c) return;
      setData(empty()); setPages([]); setRefresh(n => n + 1);
    } catch (e) {
      if (active.current === c) { setUncertain(true); setError(e instanceof Error && e.name === "Error" ? e.message : "更新结果尚未确认，请刷新收件箱核对；不会自动重试。"); }
    } finally { if (active.current === c) { active.current = null; setBusy(false); } }
  }
  const locked = busy || loading || uncertain;
  return <section aria-label="提醒收件箱">
    <h3>已收到的提醒</h3>
    <label><input type="checkbox" checked={archived} disabled={busy || loading} onChange={e => { setData(empty()); setPages([]); setArchived(e.target.checked); }} />查看已归档提醒</label>
    <button disabled={busy || loading} onClick={() => setRefresh(n => n + 1)}>刷新收件箱</button>
    {loading && <p role="status">正在读取收件箱…</p>}
    {error && <p role="alert">{error}</p>}
    {uncertain && <p role="status">请先刷新核对保存结果，再继续修改。</p>}
    {!loading && !error && data.items.length === 0 && <p>{archived ? "暂无已归档提醒。" : "暂无已投递提醒。"}</p>}
    <ul>{data.items.map(item => <li key={item.request_id}>
      <h4>{item.title}</h4><p className="memoryText">{item.body}</p>
      <p>投递于 {new Date(Number(item.delivered_at_unix_ms)).toLocaleString()} · {item.read_at_unix_ms ? "已读" : "未读"}{item.archived_at_unix_ms ? " · 已归档" : ""}</p>
      <button disabled={locked} onClick={() => void update(item, !item.read_at_unix_ms, Boolean(item.archived_at_unix_ms))}>{item.read_at_unix_ms ? "标记未读" : "标记已读"}</button>
      <button disabled={locked} onClick={() => void update(item, Boolean(item.read_at_unix_ms), !item.archived_at_unix_ms)}>{item.archived_at_unix_ms ? "恢复到收件箱" : "归档提醒"}</button>
    </li>)}</ul>
    <div className="pagination"><button disabled={locked || pages.length === 0} onClick={() => setPages(p => p.slice(0, -1))}>上一页已收提醒</button><button disabled={locked || !data.next_cursor} onClick={() => setPages(p => [...p, data.next_cursor!])}>下一页已收提醒</button></div>
  </section>;
}
