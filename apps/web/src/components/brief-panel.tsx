"use client";

import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";

import { changingFeedValueInputs } from "./feed-value-events";
import { BriefSchedule } from "./brief-schedule";

type Preferences = { revision: string; keywords: string[] };
type Summary = { request_id: string; day_start_unix_ms: string; created_at_unix_ms: string; status: "ready" | "deleted" | "invalidated" };
type Brief = Summary & { preference_revision: string; plan: null | { keywords: string[]; candidate_count: number; omitted_count: number; items: { entry: { subscription_id: string; entry_key: string; title: string; summary: string; link: string | null }; score: number; freshness_points: number; matches: { keyword: string; in_title: boolean; points: number }[] }[] } };
type History = { items: Summary[]; next_cursor: string | null };
type Operation = { kind: "preferences" | "generate" | "delete"; path: string; lookup: string; method: string; body: object };
const empty = (): History => ({ items: [], next_cursor: null });
const statuses = { ready: "已生成", deleted: "已删除", invalidated: "来源已删除，日报已失效" };
function date(value: string) { const d = new Date(Number(value)); return Number.isFinite(d.getTime()) ? d.toISOString().slice(0, 10) : "日期不可显示"; }
function safeLink(value: string | null) {
  try { const url = new URL(value ?? ""); return url.protocol === "https:" && !url.username && !url.password ? url.href : undefined; } catch { return undefined; }
}
function message(status: number) {
  if (status === 401) return "登录已失效，请重新登录。";
  if (status === 404) return "尚未找到该日报，或当前账户无权访问。";
  if (status === 409) return "偏好版本已变化、候选条目超限或日报额度已满。请核对并刷新。";
  if ([400, 413, 422].includes(status)) return "请检查关键词：最多 5 个，每个最多 64 字，不能重复或包含控制字符。";
  return "服务暂不可用，操作可能已保存，请核对原操作。";
}
export function BriefPanel() {
  const [preferences, setPreferences] = useState<Preferences | null>(null);
  const [keywords, setKeywords] = useState("");
  const [history, setHistory] = useState<History>(empty);
  const [pages, setPages] = useState<string[]>([]);
  const [selected, setSelected] = useState<Brief | null>(null);
  const [pending, setPending] = useState<Operation | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [ready, setReady] = useState(false);
  const [expired, setExpired] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const active = useRef<AbortController | null>(null);
  const alive = useRef(true);
  const sessionExpired = useRef(false);
  const expire = useCallback(() => { sessionExpired.current = true; setExpired(true); setReady(false); setPreferences(null); setKeywords(""); setHistory(empty()); setPages([]); setSelected(null); setPending(null); setDeleting(false); setNotice(""); active.current?.abort(); active.current = null; setBusy(false); setError("登录已失效，请重新登录。"); }, []);
  const valid = (c: AbortController) => alive.current && active.current === c && !c.signal.aborted;
  async function request<T>(path: string, c: AbortController, operation?: Operation): Promise<T> {
    const response = await fetch(path, { method: operation?.method ?? "GET", cache: "no-store", headers: operation ? { "Content-Type": "application/json", "X-Requested-With": "personal-ai" } : undefined, body: operation ? JSON.stringify(operation.body) : undefined, signal: AbortSignal.any([c.signal, AbortSignal.timeout(10000)]) });
    if (!response.ok) {
      if (response.status === 401 && valid(c)) {
        expire();
      }
      throw new Error(message(response.status));
    }
    return response.json();
  }
  async function run(work: (c: AbortController) => Promise<void>) {
    if (active.current) return;
    const c = new AbortController(); active.current = c; setBusy(true); setError("");
    try { await work(c); }
    catch (e) { if (valid(c)) setError(e instanceof Error && e.name === "Error" ? e.message : "连接中断，结果尚未确认。请核对或重试原操作。"); }
    finally { if (valid(c)) { active.current = null; setBusy(false); } }
  }
  async function list(c: AbortController, cursors: string[]) {
    const data = await request<History>(`/api/feed-briefs${cursors.length ? `?after=${encodeURIComponent(cursors.at(-1)!)}` : ""}`, c);
    if (valid(c)) { setHistory(data); setPages(cursors); }
  }
  async function load(c: AbortController) {
    setSelected(null); setDeleting(false); setReady(false);
    const pref = await request<Preferences>("/api/feed-brief-preferences", c);
    await list(c, []);
    if (valid(c)) { setPreferences(pref); setKeywords(pref.keywords.join("\n")); setReady(true); }
  }
  useEffect(() => {
    alive.current = true;
    const start = window.setTimeout(() => void run(load), 0);
    const changed = () => {
      if (sessionExpired.current) return;
      active.current?.abort(); active.current = null; setBusy(false); setSelected(null); setDeleting(false);
      setNotice("订阅已删除，请重新查询日报状态。");
      void run(load);
    };
    window.addEventListener("feed-sources-changed", changed);
    return () => { window.clearTimeout(start); alive.current = false; active.current?.abort(); active.current = null; window.removeEventListener("feed-sources-changed", changed); };
    // AccountPanel remounts this component for each authenticated user.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  async function mutate(operation: Operation) {
    await run(async c => {
      setPending(operation); setNotice(""); setDeleting(false);
      const readResult = () => request<Preferences | Brief>(operation.path, c, operation);
      const result = await (operation.kind === "preferences" ? changingFeedValueInputs(readResult, () => valid(c)) : readResult());
      if (!valid(c)) return;
      setPending(null);
      if (operation.kind === "preferences") { const p = result as Preferences; setPreferences(p); setKeywords(p.keywords.join("\n")); }
      else if (operation.kind === "generate") setSelected(result as Brief);
      else setSelected(null);
      setNotice(operation.kind === "preferences" ? "日报偏好已保存。" : operation.kind === "delete" ? "日报正文已删除。" : "日报已保存，可从历史记录再次查看。");
      await list(c, []);
    });
  }
  function save(e: FormEvent) {
    e.preventDefault(); if (!preferences || pending) return;
    void mutate({ kind: "preferences", method: "PUT", path: "/api/feed-brief-preferences", lookup: "/api/feed-brief-preferences", body: { revision: preferences.revision, keywords: keywords.trim() ? keywords.split("\n") : [] } });
  }
  function inspect(path: string, recover = false) {
    void run(async c => {
      setSelected(null); setDeleting(false);
      const result = await request<Preferences | Brief>(path, c);
      if (!valid(c)) return;
      if (path === "/api/feed-brief-preferences") { const p = result as Preferences; setPreferences(p); setKeywords(p.keywords.join("\n")); }
      else setSelected(result as Brief);
      if (recover) { setPending(null); setNotice("已查询当前保存结果，请核对内容；查询不会生成新日报。"); }
    });
  }
  const locked = busy || !ready || expired || pending !== null;
  const dirty = preferences !== null && keywords !== preferences.keywords.join("\n");
  return <section className="feedPanel briefPanel" aria-label="Daily Brief 日报">
    <h2>Daily Brief 日报</h2>
    {!expired && <BriefSchedule onExpired={expire} />}
    <p>按关键词与新鲜度整理已保存的 RSS 条目。不会采集新内容、调用模型或发送通知。</p>
    <form onSubmit={save}>
      <label htmlFor="brief-keywords">日报关键词（每行一个，最多 5 个）</label>
      <textarea id="brief-keywords" rows={3} maxLength={324} value={keywords} disabled={locked} onChange={e => setKeywords(e.target.value)} />
      <p>留空只按新鲜度排序。每个词最多 64 字，不区分大小写。</p>
      <button disabled={locked}>保存日报偏好</button>
    </form>
    {dirty && <p>请先保存关键词，再生成日报。</p>}
    <button disabled={locked || dirty} onClick={() => { if (!preferences) return; const request_id = crypto.randomUUID(); void mutate({ kind: "generate", path: "/api/feed-briefs", lookup: `/api/feed-briefs/${request_id}`, method: "POST", body: { request_id, preference_revision: preferences.revision } }); }}>生成今日日报</button>
    <p>以 UTC 日期统计当天首次发现的条目。每天最多生成 10 份，每份最多 20 条；删除不会恢复额度。</p>
    <button disabled={busy || expired} onClick={() => void run(load)}>刷新日报</button>
    {busy && <p role="status">正在处理日报…</p>}{error && <p role="alert">{error}</p>}{notice && <p role="status">{notice}</p>}
    {pending && <div className="feedReview" aria-label="核对日报操作"><p>结果未确认，请查询或重试原操作。</p>
      <button disabled={busy || expired} onClick={() => inspect(pending.lookup, true)}>核对原日报操作</button>
      <button disabled={busy || expired} onClick={() => void mutate(pending)}>重试原日报操作</button>
      <button disabled={busy || expired} onClick={() => { setPending(null); setNotice("已关闭提示。已保存的日报仍可从历史查询。"); }}>关闭日报核对提示</button>
    </div>}
    <h3>日报历史</h3>{ready && history.items.length === 0 && <p>暂无日报。</p>}
    <ul className="feedList" aria-label="日报历史">{history.items.map(item => <li key={item.request_id}><p>{date(item.day_start_unix_ms)}（UTC） · {statuses[item.status]}</p><button disabled={locked} onClick={() => inspect(`/api/feed-briefs/${item.request_id}`)}>查看日报</button></li>)}</ul>
    <button disabled={locked || !pages.length} onClick={() => void run(c => list(c, pages.slice(0, -1)))}>上一页日报</button>
    <button disabled={locked || !history.next_cursor} onClick={() => void run(c => list(c, [...pages, history.next_cursor!]))}>下一页日报</button>
    {selected && <div className="feedReview" aria-label="日报详情"><h3>{date(selected.day_start_unix_ms)}（UTC） · {statuses[selected.status]}</h3>
      <button disabled={locked} onClick={() => inspect(`/api/feed-briefs/${selected.request_id}`)}>更新日报状态</button>
      {selected.plan ? <><p>本份关键词：{selected.plan.keywords.join("、") || "未设置"}；候选 {selected.plan.candidate_count} 条，排名或来源上限省略 {selected.plan.omitted_count} 条。</p>
        {selected.plan.items.length === 0 && <p>当天暂无符合条件的已保存条目。可先到 RSS 订阅中查看采集结果。</p>}
        <ol className="feedList">{selected.plan.items.map(item => <li key={`${item.entry.subscription_id}/${item.entry.entry_key}`}><h4>{item.entry.title || "无标题条目"}</h4><p>{item.entry.summary}</p><p>评分 {item.score}；新鲜度 {item.freshness_points} 分{item.matches.map(m => `；${m.keyword}：${m.in_title ? "标题" : "摘要"} +${m.points}`).join("")}</p>{safeLink(item.entry.link) && <a href={safeLink(item.entry.link)} target="_blank" rel="noopener noreferrer" referrerPolicy="no-referrer">打开来源链接</a>}</li>)}</ol>
      </> : <p>正文已清除。此记录不会重新生成或恢复正文。</p>}
      {selected.status !== "deleted" && <button disabled={locked} onClick={() => setDeleting(true)}>删除日报</button>}
      {deleting && <div><p>确认清除这份日报正文？历史记录保留，不能恢复。</p><button disabled={locked} onClick={() => { const path = `/api/feed-briefs/${selected.request_id}`; void mutate({ kind: "delete", path, lookup: path, method: "DELETE", body: {} }); }}>确认删除日报</button><button disabled={locked} onClick={() => setDeleting(false)}>保留日报</button></div>}
    </div>}
  </section>;
}
