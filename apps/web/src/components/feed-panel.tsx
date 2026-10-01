"use client";

import { useEffect, useRef, useState, type FormEvent } from "react";

type Subscription = { name: string; snapshot: { subscription_id: string; revision: string; source_url: string; enabled: boolean } };
type Collection = { plan: { request_id: string; subscription_id: string; source_url: string; subscription_revision: string; approval_expires_at_unix_ms: string }; digest: string; status: string; reason: string | null; counts: { inserted: number; updated: number; unchanged: number } };
type Entry = { entry_key: string; title: string; summary: string; link: string | null; published_at: string | null };
type Audit = { event: string; at_unix_ms: string; reason: string | null };
type Page<T> = { items: T[]; next_cursor: string | null };
type Operation = { path: string; method: string; body: object; kind: "subscription" | "delete" | "preview" | "confirm" | "cancel" | "recover"; lookup: string };
const empty = <T,>(): Page<T> => ({ items: [], next_cursor: null });
const statuses: Record<string, string> = { draft: "待确认", running: "采集中", succeeded: "采集完成", failed: "采集失败", unknown: "结果未知", cancelled: "已取消" };
const reasons: Record<string, string> = { execution_expired: "执行超时，结果未确认", subscription_changed: "订阅已变更，旧结果未写入", parse: "来源内容不符合受限 RSS 格式", unknown: "来源可能已收到请求", transport: "来源请求未通过传输检查", expired: "执行已过期" };
function message(status: number) {
  if (status === 401) return "登录已失效，请重新登录。";
  if (status === 404) return "记录不存在、已删除或无权访问。";
  if (status === 409) return "状态或订阅版本已变化、预览已过期或额度已满，请刷新后重新审阅。";
  if (status === 429) return "采集繁忙，请稍后核对原请求。";
  if (status === 400 || status === 422) return "请检查输入：名称最多 120 字，来源须为公网 HTTPS RSS 地址。";
  return "服务暂不可用；操作可能已完成，请核对原记录。";
}
function safeLink(value: string | null) {
  try { const url = new URL(value ?? ""); return ["https:", "http:"].includes(url.protocol) && !url.username && !url.password ? url.href : undefined; } catch { return undefined; }
}

export function FeedPanel() {
  const [subscriptions, setSubscriptions] = useState<Page<Subscription>>(empty);
  const [history, setHistory] = useState<Page<Collection>>(empty);
  const [entries, setEntries] = useState<Page<Entry>>(empty);
  const [subPages, setSubPages] = useState<string[]>([]);
  const [historyPages, setHistoryPages] = useState<string[]>([]);
  const [entryPages, setEntryPages] = useState<string[]>([]);
  const [entryLoaded, setEntryLoaded] = useState(false);
  const [entrySub, setEntrySub] = useState<Subscription | null>(null);
  const [selected, setSelected] = useState<Collection | null>(null);
  const [audit, setAudit] = useState<Audit[]>([]);
  const [editing, setEditing] = useState<Subscription | null>(null);
  const [deleting, setDeleting] = useState<Subscription | null>(null);
  const [name, setName] = useState("");
  const [source, setSource] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [publicMode, setPublicMode] = useState(false);
  const [ack, setAck] = useState(false);
  const [busy, setBusy] = useState(false);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [pending, setPending] = useState<Operation | null>(null);
  const active = useRef<AbortController | null>(null);
  const alive = useRef(true);

  async function request<T>(path: string, controller: AbortController, operation?: Operation): Promise<T> {
    const response = await fetch(path, { method: operation?.method ?? "GET", cache: "no-store", headers: operation ? { "Content-Type": "application/json", "X-Requested-With": "personal-ai" } : undefined, body: operation ? JSON.stringify(operation.body) : undefined, signal: AbortSignal.any([controller.signal, AbortSignal.timeout(operation?.kind === "confirm" ? 30000 : 10000)]) });
    if (!response.ok) {
      if (response.status === 401 && alive.current && !controller.signal.aborted) {
        setReady(false); setSubscriptions(empty()); setHistory(empty()); setEntries(empty()); setSelected(null); setAudit([]); setEntrySub(null); setEditing(null); setDeleting(null); setName(""); setSource(""); setPending(null); setAck(false);
      }
      throw new Error(message(response.status));
    }
    return response.json();
  }
  async function run(work: (controller: AbortController) => Promise<void>) {
    if (active.current) return;
    const controller = new AbortController(); active.current = controller; setBusy(true); setError("");
    try { await work(controller); }
    catch (e) { if (alive.current && active.current === controller) setError(e instanceof Error && e.name === "Error" ? e.message : "连接中断，结果尚未确认。请核对原记录。"); }
    finally { if (alive.current && active.current === controller) { active.current = null; setBusy(false); } }
  }
  async function load(controller: AbortController, subs: string[], collections: string[]) {
    setAck(false);
    const suffix = (pages: string[]) => pages.length ? `?after=${encodeURIComponent(pages.at(-1)!)}` : "";
    const [config, s, h] = await Promise.all([
      request<{ execution_enabled: boolean }>("/api/feeds/config", controller),
      request<Page<Subscription>>(`/api/feed-subscriptions${suffix(subs)}`, controller),
      request<Page<Collection>>(`/api/feed-collections${suffix(collections)}`, controller),
    ]);
    if (!alive.current || controller.signal.aborted) return;
    setPublicMode(config.execution_enabled); setSubscriptions(s); setHistory(h); setSubPages(subs); setHistoryPages(collections); setReady(true);
  }
  useEffect(() => {
    alive.current = true;
    void run(c => load(c, [], []));
    return () => { alive.current = false; active.current?.abort(); active.current = null; };
    // Initial load only; subsequent reads are explicit and serialized with mutations.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function mutate(operation: Operation) {
    await run(async c => {
      setPending(operation); setAck(false); setNotice("");
      const result = await request<Collection | Subscription>(operation.path, c, operation);
      if (!alive.current || c.signal.aborted) return;
      setPending(null); setDeleting(null);
      if (["preview", "confirm", "cancel", "recover"].includes(operation.kind)) { setSelected(result as Collection); setAudit([]); }
      else { setEditing(null); setName(""); setSource(""); setEnabled(true); setSelected(null); setAudit([]); setEntrySub(null); setEntries(empty()); }
      setNotice(operation.kind === "preview" ? "预览已保存，尚未请求来源。" : "操作已保存，请查看当前状态。");
      await load(c, [], []);
    });
  }
  function save(event: FormEvent) {
    event.preventDefault(); if (pending) return;
    const id = editing?.snapshot.subscription_id ?? crypto.randomUUID();
    const path = `/api/feed-subscriptions/${id}`;
    void mutate({ kind: "subscription", method: editing ? "PUT" : "POST", path: editing ? path : "/api/feed-subscriptions", lookup: path, body: { ...(editing ? { revision: editing.snapshot.revision } : { id }), name, source_url: source, enabled } });
  }
  function preview(sub: Subscription) {
    const id = crypto.randomUUID();
    void mutate({ kind: "preview", method: "POST", path: `/api/feed-subscriptions/${sub.snapshot.subscription_id}/collections`, lookup: `/api/feed-collections/${id}`, body: { request_id: id } });
  }
  function act(kind: "confirm" | "cancel" | "recover") {
    if (!selected || pending || (kind === "confirm" && (!ack || !publicMode))) return;
    const path = `/api/feed-collections/${selected.plan.request_id}`;
    void mutate({ kind, method: "POST", path: `${path}/${kind}`, lookup: path, body: kind === "confirm" ? { accepted_digest: selected.digest, acknowledge_source_request: true } : {} });
  }
  function inspect(path: string, recovery = false) {
    void run(async c => {
      setAck(false);
      const saved = await request<Collection | Subscription>(path, c);
      if (!alive.current || c.signal.aborted) return;
      if (path.startsWith("/api/feed-collections/")) {
        setSelected(saved as Collection); setAudit([]);
        if (recovery) { setPending(null); setNotice("已读取原请求。查询不会重新采集，请重新审阅当前状态。"); }
        const events = await request<Audit[]>(`${path}/audit`, c);
        if (alive.current && !c.signal.aborted) setAudit(events);
      } else {
        setEditing(saved as Subscription); setName((saved as Subscription).name); setSource((saved as Subscription).snapshot.source_url); setEnabled((saved as Subscription).snapshot.enabled);
        if (recovery) { setPending(null); setNotice("已读取原订阅，请核对保存结果。"); }
      }
    });
  }
  function readEntries(sub: Subscription, pages: string[]) {
    void run(async c => {
      setEntries(empty()); setEntryLoaded(false); setEntryPages(pages); setEntrySub(sub);
      const saved = await request<Page<Entry>>(`/api/feed-subscriptions/${sub.snapshot.subscription_id}/entries${pages.length ? `?after=${encodeURIComponent(pages.at(-1)!)}` : ""}`, c);
      if (alive.current && !c.signal.aborted) { setEntries(saved); setEntryLoaded(true); setEntryPages(pages); }
    });
  }
  const locked = busy || !ready || pending !== null;
  return <section className="feedPanel" aria-label="RSS 订阅">
    <h2>RSS 订阅</h2><p>手动采集 RSS 2.0，先预览再确认。不抓取文章、图片或附件，不调用模型。</p>
    <p>{ready ? publicMode ? "公网采集已启用，每次仍需明确确认。" : "公网采集已关闭，仍可管理订阅和保存预览。" : "尚未读取 RSS 配置。"}</p>
    <form onSubmit={save}>
      <h3>{editing ? "编辑订阅" : "添加订阅"}</h3>
      <label htmlFor="feed-name">订阅名称</label><input id="feed-name" required maxLength={120} value={name} disabled={locked} onChange={e => setName(e.target.value)} />
      <label htmlFor="feed-source">RSS 来源地址</label><input id="feed-source" type="url" required maxLength={2048} placeholder="https://example.com/rss" value={source} disabled={locked} onChange={e => setSource(e.target.value)} />
      <label><input type="checkbox" checked={enabled} disabled={locked} onChange={e => setEnabled(e.target.checked)} />启用订阅</label>
      <button disabled={locked}>{editing ? "保存订阅" : "添加订阅"}</button>
      {editing && <button type="button" disabled={locked} onClick={() => { setEditing(null); setName(""); setSource(""); setEnabled(true); }}>放弃编辑</button>}
    </form>
    {error && <p role="alert">{error}</p>}{notice && <p role="status">{notice}</p>}
    {pending && <div className="feedReview" aria-label="核对 RSS 操作"><p>操作结果未确认。请核对原记录；不要新建替代采集。</p><code>{pending.lookup.split("/").at(-1)}</code><button disabled={busy} onClick={() => inspect(pending.lookup, true)}>核对原记录</button>
      {pending.kind !== "confirm" && <button disabled={busy} onClick={() => void mutate(pending)}>重试原管理操作</button>}
      <button disabled={busy} onClick={() => { setPending(null); setAck(false); setSelected(null); setAudit([]); setNotice("已关闭提示；可从订阅或采集历史继续核对。此操作不会撤回来源请求。"); }}>关闭核对提示</button>
    </div>}
    <button disabled={busy} onClick={() => void run(c => load(c, subPages, historyPages))}>刷新 RSS</button>
    {busy && <p role="status">正在处理 RSS…</p>}
    <h3>我的订阅</h3>{ready && subscriptions.items.length === 0 && <p>暂无订阅。</p>}
    <ul className="feedList">{subscriptions.items.map(sub => <li key={sub.snapshot.subscription_id}><h4>{sub.name}</h4><p>{sub.snapshot.source_url}</p><p>{sub.snapshot.enabled ? "已启用" : "已停用"} · 版本 {sub.snapshot.revision}</p>
      <button disabled={locked} onClick={() => { setEditing(sub); setName(sub.name); setSource(sub.snapshot.source_url); setEnabled(sub.snapshot.enabled); setAck(false); }}>编辑</button>
      <button disabled={locked || !sub.snapshot.enabled} onClick={() => preview(sub)}>预览采集</button>
      <button disabled={locked} onClick={() => readEntries(sub, [])}>查看条目</button>
      <button disabled={locked} onClick={() => setDeleting(sub)}>删除订阅</button>
    </li>)}</ul>
    <div className="pagination"><button disabled={locked || !subPages.length} onClick={() => void run(c => load(c, subPages.slice(0, -1), historyPages))}>上一页订阅</button><button disabled={locked || !subscriptions.next_cursor} onClick={() => void run(c => load(c, [...subPages, subscriptions.next_cursor!], historyPages))}>下一页订阅</button></div>
    {deleting && <div className="feedReview" aria-label="删除 RSS 订阅"><p>删除「{deleting.name}」及已保存条目？采集历史保留，已发送的请求无法撤回。</p><button disabled={locked} onClick={() => { const path = `/api/feed-subscriptions/${deleting.snapshot.subscription_id}`; void mutate({ kind: "delete", method: "DELETE", path, lookup: path, body: { revision: deleting.snapshot.revision } }); }}>确认删除订阅</button><button disabled={locked} onClick={() => setDeleting(null)}>保留订阅</button></div>}
    {entrySub && <div className="feedReview" aria-label="RSS 条目"><h3>{entrySub.name} · 已保存条目</h3>{entryLoaded && entries.items.length === 0 && !busy && <p>暂无已保存条目。</p>}<ul>{entries.items.map(entry => <li key={entry.entry_key}><h4>{entry.title}</h4><p className="feedText">{entry.summary}</p>{entry.published_at && <p>{entry.published_at}</p>}{safeLink(entry.link) && <a href={safeLink(entry.link)} target="_blank" rel="noopener noreferrer" referrerPolicy="no-referrer">打开来源文章（离开本站）</a>}</li>)}</ul><div className="pagination"><button disabled={locked || !entryPages.length} onClick={() => readEntries(entrySub, entryPages.slice(0, -1))}>上一页条目</button><button disabled={locked || !entries.next_cursor} onClick={() => readEntries(entrySub, [...entryPages, entries.next_cursor!])}>下一页条目</button></div></div>}
    <h3>采集历史</h3>{ready && history.items.length === 0 && <p>暂无采集记录。</p>}
    <ul className="feedList">{history.items.map(item => <li key={item.plan.request_id}><p>{item.plan.source_url}</p><p>{statuses[item.status] ?? item.status}</p><button disabled={locked} onClick={() => inspect(`/api/feed-collections/${item.plan.request_id}`)}>审阅采集</button></li>)}</ul>
    <div className="pagination"><button disabled={locked || !historyPages.length} onClick={() => void run(c => load(c, subPages, historyPages.slice(0, -1)))}>上一页采集</button><button disabled={locked || !history.next_cursor} onClick={() => void run(c => load(c, subPages, [...historyPages, history.next_cursor!]))}>下一页采集</button></div>
    {selected && <div className="feedReview" aria-label="RSS 采集确认"><h3>{statuses[selected.status] ?? selected.status}</h3><p>来源：{selected.plan.source_url}</p><p>订阅版本：{selected.plan.subscription_revision}</p><p>请求：<code>{selected.plan.request_id}</code></p><p>一次来源 GET · 最多 1 MiB / 100 条 · 8 秒 · 不跟随重定向</p><p>新增 {selected.counts.inserted} · 更新 {selected.counts.updated} · 未变化 {selected.counts.unchanged}</p>{selected.reason && <p>原因：{reasons[selected.reason] ?? "采集未完成，请核对原请求"}</p>}
      {selected.status === "draft" && <><p>确认期限：{new Date(Number(selected.plan.approval_expires_at_unix_ms)).toLocaleString()}</p><label><input type="checkbox" checked={ack} disabled={locked || !publicMode} onChange={e => setAck(e.target.checked)} />我已核对来源，同意向该站点发送一次请求</label><button disabled={locked || !ack || !publicMode} onClick={() => act("confirm")}>确认采集一次</button><button disabled={locked} onClick={() => act("cancel")}>取消采集预览</button></>}
      {selected.status === "running" && <><p>请求可能已发送。查询不会重新采集；执行期限超过 60 秒后可标记未知。</p><button disabled={locked} onClick={() => act("recover")}>恢复超时状态</button></>}
      {selected.status === "unknown" && <p>来源可能已收到请求，不会自动重试。</p>}
      <button disabled={locked} onClick={() => inspect(`/api/feed-collections/${selected.plan.request_id}`)}>查询原请求</button>
      <h4>采集审计</h4><ul>{audit.map((event, i) => <li key={i}>{statuses[event.event] ?? "状态变更"} · {new Date(Number(event.at_unix_ms)).toLocaleString()}{event.reason ? ` · ${reasons[event.reason] ?? "结果未确认"}` : ""}</li>)}</ul>
    </div>}
  </section>;
}
