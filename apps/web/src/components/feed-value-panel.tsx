"use client";

import { useEffect, useRef, useState } from "react";
import { connection, type Connection } from "./subscription-connection-panel";
import { audit, date, detail, page, statuses, summary, type Audit, type ValueDetail, type ValueSummary } from "./feed-value-types";

const endpoint = "/api/feed-values";
type Preview = { id: string; connection_id: string; connection_revision: string; model: string };
class HttpError extends Error {
  constructor(public status: number) {
    super(status === 401 ? "登录已失效，请退出后重新登录。" : status === 404 ? "原请求不存在或无权查看。" : status === 409 ? "候选、连接、摘要或状态已变化，请核对原请求；没有候选时请先采集 RSS 并设置日报关键词。" : status === 403 ? "请求未通过安全检查，请重新登录后核对。" : "服务暂不可用或输入无效，请核对原请求。");
  }
}
export function FeedValuePanel() {
  const [connections, setConnections] = useState<Connection[]>([]);
  const [connectionNext, setConnectionNext] = useState<string | null>(null);
  const [selected, setSelected] = useState("");
  const [model, setModel] = useState("");
  const [items, setItems] = useState<ValueSummary[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [review, setReview] = useState<ValueDetail | null>(null);
  const [events, setEvents] = useState<Audit[]>([]);
  const [share, setShare] = useState(false);
  const [usage, setUsage] = useState(false);
  const [pending, setPending] = useState<Preview | null>(null);
  const [uncertain, setUncertain] = useState<string | null>(null);
  const [missing, setMissing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [expired, setExpired] = useState(false);
  const [error, setError] = useState("");
  const [now, setNow] = useState(0);
  const active = useRef<AbortController | null>(null);
  useEffect(() => {
    let mounted = true;
    queueMicrotask(() => { if (mounted) setNow(Date.now()); });
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => { mounted = false; active.current?.abort(); active.current = null; clearInterval(timer); };
  }, []);
  const locked = busy || expired || !!uncertain;
  const chosen = connections.find(c => c.id === selected);
  function clearReview() { setReview(null); setEvents([]); setShare(false); setUsage(false); }
  async function perform(work: (request: (path: string, body?: unknown) => Promise<unknown>) => Promise<void>) {
    if (active.current || expired) return;
    const controller = new AbortController(); active.current = controller; setBusy(true); setError("");
    async function request(path: string, body?: unknown) {
      const response = await fetch(path, { cache: "no-store", method: body === undefined ? "GET" : "POST", ...(body === undefined ? {} : { headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" }, body: JSON.stringify(body) }), signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) });
      if (active.current !== controller) throw new Error("请求已停止。");
      if (!response.ok) throw new HttpError(response.status);
      const result: unknown = await response.json();
      if (active.current !== controller) throw new Error("请求已停止。");
      return result;
    }
    try { await work(request); }
    catch (e) {
      if (active.current !== controller) return;
      if (e instanceof HttpError && e.status === 401) {
        setExpired(true); setConnections([]); setConnectionNext(null); setSelected(""); setModel(""); setItems([]); setNext(null); clearReview(); setPending(null); setUncertain(null);
      }
      setError(e instanceof Error && e.name === "Error" ? e.message : "响应未确认，请核对原请求；不会自动重试。");
    } finally { if (active.current === controller) { active.current = null; setBusy(false); } }
  }
  function loadConnections(after: string | null) {
    if (active.current || locked) return;
    setConnections([]); setConnectionNext(null); setSelected(""); setModel("");
    void perform(async request => {
      const result = page(await request(`/api/subscription-connections${after ? `?after=${encodeURIComponent(after)}` : ""}`), connection, 10);
      setConnections(result.items); setConnectionNext(result.next_cursor);
    });
  }
  function loadHistory(after: string | null) {
    if (active.current || locked) return;
    clearReview(); setItems([]); setNext(null); setLoaded(false);
    void perform(async request => {
      const result = page(await request(endpoint + (after ? `?after=${encodeURIComponent(after)}` : "")), summary, 20);
      setItems(result.items); setNext(result.next_cursor); setLoaded(true);
    });
  }
  function inspect(id: string) {
    if (active.current || busy || expired) return;
    clearReview(); setMissing(false);
    void perform(async request => {
      let saved: ValueDetail;
      try { saved = detail(await request(`${endpoint}/${id}`), id); }
      catch (e) { if (e instanceof HttpError && e.status === 404) { setMissing(true); setItems(previous => previous.filter(i => i.id !== id)); } throw e; }
      setReview(saved); setUncertain(null); setPending(null);
      setItems(previous => previous.map(i => i.id === id ? saved : i));
    });
  }
  function preview(original?: Preview) {
    if (active.current || busy || expired || (!original && (uncertain || !chosen || chosen.status !== "active" || Number(chosen.valid_until_unix_ms) <= now || !chosen.models.includes(model)))) return;
    const input = original ?? { id: crypto.randomUUID(), connection_id: chosen!.id, connection_revision: chosen!.revision, model };
    setPending(input); setUncertain(input.id); setMissing(false); clearReview();
    void perform(async request => {
      const saved = detail(await request(endpoint, input), input.id);
      setReview(saved); setPending(null); setUncertain(null);
    });
  }
  function mutate(kind: "approve" | "cancel") {
    if (active.current || locked || !review || review.pricing.kind !== "subscription" || (kind === "approve" && (!share || !usage || review.status !== "draft" || review.pricing.kind !== "subscription" || Number(review.expires_at_unix_ms) <= now || Number(review.pricing.valid_until_unix_ms) <= now))) return;
    const original = review;
    setUncertain(original.id); setMissing(false); clearReview();
    void perform(async request => {
      const saved = detail(await request(`${endpoint}/${original.id}/${kind}`, kind === "cancel" ? {} : { digest: original.digest, acknowledge_sharing: true, acknowledge_subscription_usage: true }), original.id);
      if (kind === "approve" && !["authorized", "running", "succeeded"].includes(saved.status)) throw new Error("批准结果不完整，请核对原请求。");
      if (kind === "cancel" && saved.status !== "cancelled") throw new Error("取消结果不完整，请核对原请求。");
      setReview(saved); setUncertain(null); setItems(previous => previous.map(i => i.id === saved.id ? saved : i));
    });
  }
  const canApprove = review?.status === "draft" && review.pricing.kind === "subscription" && Number(review.expires_at_unix_ms) > now && Number(review.pricing.valid_until_unix_ms) > now;
  return <section className="feedPanel feedValuePanel" aria-label="RSS 价值评分">
    <p className="kicker">RSS / VALUE</p><h3>RSS 价值评分</h3>
    <p>分享当前候选和日报关键词，获取模型的参考评分。先在本机绑定订阅连接，再预览并确认分享内容和订阅用量。</p>
    <p>网页批准后不会自动运行。请在本机显式执行，再回来查询结果；评分不会替换当前日报的关键词规则排序。</p>
    {error && <p role="alert">{error}</p>}{busy && <p role="status">正在处理评分请求…</p>}
    <fieldset disabled={locked}>
      <legend>创建评分预览</legend>
      <button onClick={() => loadConnections(null)}>读取评分连接</button>
      {connectionNext && <button onClick={() => loadConnections(connectionNext)}>下一页评分连接</button>}
      <label>评分连接<select value={selected} onChange={e => { setSelected(e.target.value); setModel(""); }}><option value="">请选择已绑定连接</option>{connections.map(c => <option key={c.id} value={c.id} disabled={c.status !== "active" || Number(c.valid_until_unix_ms) <= now}>{c.label} · 版本 {c.revision}{c.status !== "active" || Number(c.valid_until_unix_ms) <= now ? "（不可用）" : ""}</option>)}</select></label>
      <label>评分模型<select value={model} onChange={e => setModel(e.target.value)}><option value="">请选择模型</option>{chosen?.models.map(m => <option key={m} value={m}>{m}</option>)}</select></label>
      <button disabled={!chosen || !model || chosen.status !== "active" || Number(chosen.valid_until_unix_ms) <= now} onClick={() => preview()}>预览分享内容</button>
      <small>连接每页最多 10 条。找不到连接时，请先完成本机绑定；没有候选时请先采集 RSS 并设置日报关键词。</small>
    </fieldset>
    <button disabled={locked} onClick={() => loadHistory(null)}>刷新评分记录</button>
    {next && <button disabled={locked} onClick={() => loadHistory(next)}>下一页评分记录</button>}
    <small>记录每页最多 20 条，刷新回到第一页。</small>
    {loaded && !busy && items.length === 0 && <p>暂无评分记录。</p>}
    <ul className="feedList">{items.map(item => <li key={item.id}><p>{statuses[item.status]} · {item.pricing.kind === "subscription" ? item.pricing.model : "历史 API 模式（只读）"}</p><small>{date(item.created_at_unix_ms)}</small><p>请求：<code>{item.id}</code></p><button disabled={locked} onClick={() => inspect(item.id)}>查看评分 {item.id}</button></li>)}</ul>
    {uncertain && <div className="feedReview" role="region" aria-label="待核对评分请求"><p>操作结果未确认。请核对原请求，不会自动重发或改用新摘要。</p><p>请求：<code>{uncertain}</code></p><button disabled={busy || expired} onClick={() => inspect(uncertain)}>核对原评分请求</button>
      {pending && <button disabled={busy || expired} onClick={() => preview(pending)}>重试原预览</button>}
      {missing && <button disabled={busy || expired} onClick={() => { setUncertain(null); setPending(null); setMissing(false); setError(""); }}>关闭不存在的请求</button>}
    </div>}
    {review && <div className="feedReview" role="region" aria-label="评分详情">
      <h4>{statuses[review.status]}</h4><p>请求：<code>{review.id}</code></p><p>摘要：<code>{review.digest}</code></p><p>授权期限：{date(review.expires_at_unix_ms)}</p>
      {review.pricing.kind === "subscription" ? <p>订阅模型：{review.pricing.model}<br />连接：<code>{review.pricing.connection_id}</code><br />连接版本：{review.pricing.configuration_version}</p> : <p>历史 API 模式仅可查询，网页不能批准金额用量。</p>}
      {review.shared_content ? <><h4>将分享给模型的完整内容</h4><p>以下为冻结的指令、关键词及候选标题/摘要，请先审阅。</p><h5>系统指令</h5><pre>{review.shared_content.instructions}</pre><h5>候选与关键词</h5><pre>{review.shared_content.input}</pre></> : <p>分享正文已清除。</p>}
      {review.status === "draft" && review.pricing.kind === "subscription" && <><p>执行会消耗订阅额度，或账户设置允许的 credits。这里不表示免费、额度充足或调用已经验证。</p><label className="agentConsent"><input type="checkbox" checked={share} disabled={locked || !canApprove} onChange={e => setShare(e.target.checked)} />我同意分享以上冻结内容。</label><label className="agentConsent"><input type="checkbox" checked={usage} disabled={locked || !canApprove} onChange={e => setUsage(e.target.checked)} />我同意使用所选账户的订阅额度或允许的 credits。</label><button disabled={locked || !canApprove || !share || !usage} onClick={() => mutate("approve")}>批准此次评分</button>{!canApprove && <p>当前预览不能批准，请核对状态或重新预览。</p>}</>}
      {review.status === "authorized" && <p>已保存授权。请用本机已绑定的连接执行 <code>chatgpt-connect</code> 的 <code>value-run</code> 命令，使用上面的请求标识；完成后点击“核对评分状态”。</p>}
      {review.status === "unknown" && <p>结果未知，可能已消耗用量。请核对本机记录；不要自动重派或重复创建。</p>}
      {review.scores && <><h4>模型参考评分</h4><p>评分仅为建议，不代表事实真伪；无法评分保留为空。</p><ul className="feedList">{review.scores.map(s => <li key={s.id}><h5>{review.candidates?.find(c => c.id === s.id)?.title}</h5><p>{s.score === null ? "无法评分" : `${s.score} / 100`}</p><p>{s.reason}</p></li>)}</ul></>}
      {review.pricing.kind === "subscription" && ["draft", "authorized", "running"].includes(review.status) && <><p>取消会清除分享内容；已发出的模型请求可能仍消耗用量，晚到结果会丢弃。</p><button disabled={locked} onClick={() => mutate("cancel")}>取消此次评分</button></>}
      <button disabled={locked} onClick={() => inspect(review.id)}>核对评分状态</button>
      <button disabled={locked} onClick={() => { setEvents([]); void perform(async request => { setEvents(audit(await request(`${endpoint}/${review.id}/audit`))); }); }}>读取评分审计</button>
      <ul aria-label="评分审计">{events.map((e, i) => <li key={i}>{e.event} · {date(e.at_unix_ms)}</li>)}</ul>
    </div>}
  </section>;
}
