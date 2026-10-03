"use client";

import { useCallback, useEffect, useRef, useState } from "react";

export type Connection = { id: string; label: string; revision: string; status: "active" | "expired" | "revoked"; models: string[]; valid_until_unix_ms: string };
type Review = { item: Connection; uncertain: boolean };
const endpoint = "/api/subscription-connections";
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
export function connection(value: unknown): Connection {
  const item = value as Connection;
  if (!item || typeof item.id !== "string" || !uuid.test(item.id) || typeof item.label !== "string" || !item.label || item.label.length > 64
    || typeof item.revision !== "string" || !/^[1-9]\d{0,3}$/.test(item.revision) || Number(item.revision) > 1000
    || !["active", "expired", "revoked"].includes(item.status) || !Array.isArray(item.models) || item.models.length > 100
    || item.models.some(model => typeof model !== "string" || model.length > 128)
    || typeof item.valid_until_unix_ms !== "string" || !/^[1-9]\d{0,15}$/.test(item.valid_until_unix_ms)
    || Number(item.valid_until_unix_ms) > 8.64e15) throw new Error("连接数据不完整，请重新查询。");
  return item;
}
function message(status: number) {
  if (status === 401) return "登录已失效，请退出后重新登录。";
  if (status === 403) return "请求未通过安全检查，请刷新页面后核对连接。";
  if (status === 404) return "连接不存在或无权查看。";
  if (status === 409) return "连接版本已变化，请核对最新状态后重新确认。";
  return "服务暂不可用，请稍后核对连接状态。";
}
function status(item: Connection, now: number) {
  if (item.status === "revoked") return "已撤销";
  if (item.status === "expired" || Number(item.valid_until_unix_ms) <= now) return "已到期";
  return "已绑定";
}
export function SubscriptionConnectionPanel() {
  const [items, setItems] = useState<Connection[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(true);
  const [expired, setExpired] = useState(false);
  const [review, setReview] = useState<Review | null>(null);
  const [consent, setConsent] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [now, setNow] = useState(0);
  const active = useRef<AbortController | null>(null);

  const forget = useCallback(() => { setExpired(true); setItems([]); setNext(null); setReview(null); setConsent(false); setNotice(""); }, []);
  const load = useCallback(async (controller: AbortController, after: string | null) => {
    try {
      const response = await fetch(endpoint + (after ? `?after=${encodeURIComponent(after)}` : ""), { cache: "no-store", signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) });
      if (active.current !== controller) return;
      if (response.status === 401) forget();
      if (!response.ok) throw new Error(message(response.status));
      const result = await response.json();
      if (active.current !== controller) return;
      if (!Array.isArray(result.items) || result.items.length > 10 || !(result.next_cursor === null || (typeof result.next_cursor === "string" && uuid.test(result.next_cursor)))) throw new Error("列表数据不完整，请刷新连接。");
      const records = result.items.map(connection) as Connection[];
      if (new Set(records.map(item => item.id)).size !== records.length) throw new Error("列表数据不完整，请刷新连接。");
      setItems(records); setNext(result.next_cursor); setLoaded(true); setNow(Date.now());
    } catch (e) {
      if (active.current === controller) setError(e instanceof Error && e.name === "Error" ? e.message : "无法读取连接，请刷新重试。");
    } finally {
      if (active.current === controller) { active.current = null; setBusy(false); }
    }
  }, [forget]);
  useEffect(() => {
    const controller = new AbortController(); active.current = controller;
    queueMicrotask(() => { if (!controller.signal.aborted) void load(controller, null); });
    const timer = setInterval(() => setNow(Date.now()), 30000);
    return () => { controller.abort(); active.current?.abort(); active.current = null; clearInterval(timer); };
  }, [load]);

  function refresh(after: string | null) {
    if (active.current || expired || review?.uncertain) return;
    setBusy(true); setError(""); setNotice(""); setReview(null); setConsent(false); setItems([]); setNext(null); setLoaded(false);
    const controller = new AbortController(); active.current = controller;
    void load(controller, after);
  }
  async function inspect(id: string) {
    if (active.current || expired) return;
    const controller = new AbortController(); active.current = controller;
    setBusy(true); setConsent(false); setError(""); setNotice("");
    try {
      const response = await fetch(`${endpoint}/${id}`, { cache: "no-store", signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) });
      if (active.current !== controller) return;
      if (response.status === 401) forget();
      if (response.status === 404) { setReview(null); setItems(previous => previous.filter(item => item.id !== id)); }
      if (!response.ok) throw new Error(message(response.status));
      const result = await response.json();
      if (active.current !== controller) return;
      const saved = connection(result);
      if (saved.id !== id) throw new Error("连接数据不匹配，请重新查询。");
      setReview({ item: saved, uncertain: false });
      setItems(previous => previous.map(item => item.id === id ? saved : item));
      setNotice(saved.status === "revoked" ? "已核对：此连接已撤销。" : "已读取最新状态，请重新审阅后决定是否撤销。");
    } catch (e) {
      if (active.current === controller) setError(e instanceof Error && e.name === "Error" ? e.message : "无法核对连接，请稍后重试。");
    } finally {
      if (active.current === controller) { active.current = null; setBusy(false); }
    }
  }
  async function revoke() {
    if (!review || !consent || review.uncertain || review.item.status === "revoked" || active.current || expired) return;
    const item = review.item;
    const controller = new AbortController(); active.current = controller;
    setBusy(true); setConsent(false); setError(""); setNotice(""); setReview({ item, uncertain: true });
    try {
      const response = await fetch(`${endpoint}/${item.id}/revoke`, {
        method: "POST", headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" }, body: JSON.stringify({ revision: item.revision }),
        signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]),
      });
      if (active.current !== controller) return;
      if (response.status === 401) forget();
      if (!response.ok) throw new Error(message(response.status));
      const result = await response.json();
      if (active.current !== controller) return;
      const saved = connection(result);
      if (saved.id !== item.id || saved.status !== "revoked") throw new Error("撤销结果不完整，请核对连接。");
      setItems(previous => previous.map(value => value.id === saved.id ? saved : value));
      setReview({ item: saved, uncertain: false }); setNotice("应用侧连接已撤销，相关评分授权已失效。");
    } catch (e) {
      if (active.current === controller) setError(e instanceof Error && e.name === "Error" ? e.message : "撤销结果未知，请核对连接；不会自动重试。");
    } finally {
      if (active.current === controller) { active.current = null; setBusy(false); }
    }
  }
  return <section className="feedPanel" aria-label="ChatGPT 订阅连接">
    <p className="kicker">CHATGPT / CONNECTIONS</p>
    <h3>ChatGPT 订阅连接</h3>
    <p>管理已绑定到本账户的订阅连接。绑定需要先在本机完成登录和关联；本页不接收 API Key 或登录令牌。</p>
    <p>“已绑定”仅表示连接登记有效，不代表订阅额度充足或模型调用已验证。本页不会调用模型。</p>
    <button disabled={busy || expired || !!review?.uncertain} onClick={() => refresh(null)}>刷新连接</button>
    {next && <button disabled={busy || expired || !!review?.uncertain} onClick={() => refresh(next)}>下一页连接</button>}
    <small>每页最多 10 条；刷新回到第一页。</small>
    {error && <p role="alert">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    {busy && <p role="status">正在处理连接…</p>}
    {loaded && !busy && !expired && items.length === 0 && <p>暂无订阅连接。请先通过本地连接工具完成登录和绑定。</p>}
    <ul className="feedList">{items.map(item => <li key={item.id}>
      <h4>{item.label}</h4><p>{status(item, now)} · 版本 {item.revision}</p>
      <small>登记到期：{new Date(Number(item.valid_until_unix_ms)).toLocaleString(undefined, { timeZoneName: "short" })}</small><br />
      <button disabled={busy || expired || !!review?.uncertain} onClick={() => void inspect(item.id)}>查看 {item.label}</button>
    </li>)}</ul>
    {review && <div className="feedReview" role="region" aria-label="订阅连接详情">
      <h4>{review.item.label} · {status(review.item, now)}</h4>
      <p>连接标识：<code>{review.item.id}</code><br />版本：{review.item.revision}</p>
      <p>登记的模型：{review.item.models.join("、") || "暂无"}</p>
      <p>撤销会使本应用中相关的评分授权失效，不会退出 ChatGPT 或取消 Pro 订阅。要解除 OpenAI 侧授权，请前往 <a href="https://chatgpt.com/settings/usage" target="_blank" rel="noreferrer">ChatGPT 用量设置</a>。</p>
      {review.uncertain ? <p>操作结果需要核对。不会自动重发撤销，也不会自动采用新版本。</p> : review.item.status !== "revoked" && <>
        <label className="agentConsent"><input type="checkbox" checked={consent} disabled={busy || expired} onChange={event => setConsent(event.target.checked)} />我确认撤销此应用连接及相关评分授权。</label>
        <button disabled={busy || expired || !consent} onClick={() => void revoke()}>确认撤销连接</button>
      </>}
      <button disabled={busy || expired} onClick={() => void inspect(review.item.id)}>核对连接</button>
      {!review.uncertain && <button disabled={busy} onClick={() => { setReview(null); setConsent(false); }}>关闭详情</button>}
    </div>}
  </section>;
}
