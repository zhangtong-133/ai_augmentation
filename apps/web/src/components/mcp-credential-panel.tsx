"use client";

import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";

type Credential = { id: string; host_name: string; scope: string; created_at: string; expires_at: string; revoked_at: string | null };
type Issued = { credential: Credential; token: string };
const endpoint = "/api/mcp/credentials";
function date(value: string) { return new Date(value.replace(" ", "T").replace(/([+-]\d{2})$/, "$1:00")); }
function time(value: string) { return date(value).toLocaleString(undefined, { timeZoneName: "short" }); }
function message(status: number) {
  if (status === 401) return "登录已失效，请退出后重新登录。";
  if (status === 429) return "凭据额度已满：有效凭据与过去 24 小时内签发的凭据合计最多 20 条，撤销不释放当天额度。";
  if (status === 400 || status === 422) return "请检查宿主名称、有效期和费用授权。";
  if (status === 404) return "凭据不存在或无权操作，请刷新列表核对。";
  return "服务暂不可用，请刷新列表核对操作结果。";
}

export function McpCredentialPanel() {
  const [items, setItems] = useState<Credential[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(true);
  const [expired, setExpired] = useState(false);
  const [host, setHost] = useState("");
  const [days, setDays] = useState("7");
  const [consent, setConsent] = useState(false);
  const [issued, setIssued] = useState<Issued | null>(null);
  const [revealed, setRevealed] = useState(false);
  const [unknown, setUnknown] = useState(false);
  const [reviewed, setReviewed] = useState(false);
  const [revokeId, setRevokeId] = useState<string | null>(null);
  const [revokeUnknown, setRevokeUnknown] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [now, setNow] = useState(0);
  const active = useRef<AbortController | null>(null);

  const clearSecret = useCallback(() => { setIssued(null); setRevealed(false); }, []);
  useEffect(() => {
    function hide() { if (document.visibilityState === "hidden") clearSecret(); }
    function leave() { clearSecret(); active.current?.abort(); active.current = null; setBusy(false); }
    document.addEventListener("visibilitychange", hide);
    window.addEventListener("pagehide", leave);
    return () => {
      active.current?.abort(); active.current = null;
      document.removeEventListener("visibilitychange", hide);
      window.removeEventListener("pagehide", leave);
    };
  }, [clearSecret]);

  const load = useCallback(async (controller: AbortController) => {
    try {
      const response = await fetch(endpoint, { cache: "no-store", signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) });
      if (active.current !== controller) return;
      if (response.status === 401) { setExpired(true); clearSecret(); setItems([]); }
      if (!response.ok) throw new Error(message(response.status));
      const result: { items: Credential[] } = await response.json();
      if (active.current !== controller) return;
      setItems(result.items); setLoaded(true); setReviewed(true); setError(""); setNow(Date.now());
    } catch (e) {
      if (active.current === controller) setError(e instanceof Error && e.name === "Error" ? e.message : "无法读取凭据，请刷新列表。");
    } finally {
      if (active.current === controller) { active.current = null; setBusy(false); }
    }
  }, [clearSecret]);
  useEffect(() => {
    const controller = new AbortController(); active.current = controller;
    queueMicrotask(() => { if (!controller.signal.aborted) void load(controller); });
    return () => { controller.abort(); if (active.current === controller) active.current = null; };
  }, [load]);

  function refresh() {
    if (active.current) return;
    clearSecret(); setBusy(true);
    const controller = new AbortController(); active.current = controller;
    void load(controller);
  }

  async function create(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (active.current || !consent || unknown || revokeId || issued || expired || !loaded) return;
    const controller = new AbortController(); active.current = controller;
    setBusy(true); setError(""); setNotice(""); setConsent(false); clearSecret();
    // Creation is not idempotent: once sent, an ambiguous result must never be retried automatically.
    setUnknown(true); setReviewed(false);
    try {
      const response = await fetch(endpoint, {
        method: "POST", headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" },
        body: JSON.stringify({ host_name: host.trim(), expires_in_days: Number(days), acknowledge_embedding_cost: true }),
        signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]),
      });
      if (active.current !== controller) return;
      if (!response.ok) {
        if (response.status >= 400 && response.status < 500) setUnknown(false);
        if (response.status === 401) { setExpired(true); setItems([]); }
        throw new Error(message(response.status));
      }
      const result: Issued = await response.json();
      if (active.current !== controller) return;
      if (!/^pai_mcp_[0-9a-f]{64}$/.test(result.token) || result.credential.scope !== "knowledge_search") throw new Error("未能读取完整凭据，请刷新列表核对并撤销本次授权。");
      setItems(previous => [result.credential, ...previous].slice(0, 100)); setNow(Date.now()); setUnknown(false);
      if (document.visibilityState === "visible") { setIssued(result); setNotice("凭据已创建，仅此次提供明文。"); }
      else setNotice("凭据已创建，但页面已隐藏，明文已清除；请撤销后重新签发。");
      setHost("");
    } catch (e) {
      if (active.current === controller) setError(e instanceof Error && e.name === "Error" ? e.message : "创建结果未知，请先刷新列表核对，撤销无法使用的凭据。");
    } finally {
      if (active.current === controller) { active.current = null; setBusy(false); }
    }
  }

  async function revoke() {
    if (!revokeId || active.current || expired) return;
    const id = revokeId;
    const controller = new AbortController(); active.current = controller;
    setBusy(true); setError(""); setNotice(""); clearSecret(); setRevokeUnknown(true);
    try {
      const response = await fetch(`${endpoint}/${id}/revoke`, { method: "POST", headers: { "X-Requested-With": "personal-ai" }, signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) });
      if (active.current !== controller) return;
      if (response.status === 401) { setExpired(true); setItems([]); }
      if (!response.ok) throw new Error(message(response.status));
      setItems(previous => previous.map(item => item.id === id ? { ...item, revoked_at: new Date().toISOString() } : item));
      setRevokeId(null); setRevokeUnknown(false); setNotice("凭据已撤销。已开始的检索仍可能完成。");
    } catch (e) {
      if (active.current === controller) setError(e instanceof Error && e.name === "Error" ? e.message : "撤销结果未知，可以重试撤销同一凭据。");
    } finally {
      if (active.current === controller) { active.current = null; setBusy(false); }
    }
  }

  return <section className="mcpPanel" aria-label="MCP 宿主授权">
    <p className="kicker">MCP / HOST ACCESS</p>
    <h3>MCP 宿主授权</h3>
    <p>为可信本地客户端签发独立凭据，只允许检索你的知识库。每个宿主使用自己的凭据，随时可撤销。</p>
    <p className="mcpBoundary">检索可能产生向量模型费用，没有精确金额上限；所有宿主与网页共用每日 100 次工具额度。客户端仍须在每次检索前取得你的同意。</p>
    <form onSubmit={event => void create(event)}>
      <label htmlFor="mcp-host">宿主名称</label>
      <input id="mcp-host" value={host} onChange={event => { setHost(event.target.value); setConsent(false); }} required maxLength={80} pattern=".*\S.*" autoComplete="off" disabled={busy || expired || unknown || !!issued} placeholder="例如：我的桌面客户端" />
      <label htmlFor="mcp-days">凭据有效期</label>
      <select id="mcp-days" value={days} onChange={event => { setDays(event.target.value); setConsent(false); }} disabled={busy || expired || unknown || !!issued}>
        <option value="1">1 天</option><option value="7">7 天</option><option value="30">30 天</option>
      </select>
      <label className="agentConsent"><input type="checkbox" checked={consent} onChange={event => setConsent(event.target.checked)} disabled={busy || expired || unknown || !!issued} />我信任此宿主，并同意它在上述期限内检索我的知识库及可能产生的模型费用。</label>
      <button disabled={!loaded || busy || expired || unknown || !!issued || !!revokeId || !consent || !host.trim()}>创建专用凭据</button>
    </form>
    {error && <p role="alert">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    {unknown && !busy && <div className="mcpBoundary" role="region" aria-label="核对创建结果">
      <p>本次创建结果未知，不会自动重试。请刷新列表，核对宿主和签发时间，撤销无法取得明文的凭据。</p>
      <button disabled={busy || !reviewed || expired} onClick={() => { setUnknown(false); setConsent(false); setError(""); }}>已核对列表，允许重新填写授权</button>
    </div>}
    {issued && <div className="mcpSecret" role="region" aria-label="一次性凭据">
      <h4>{issued.credential.host_name} 的一次性凭据</h4>
      <p>到期：{time(issued.credential.expires_at)}。仅在本页临时保留；刷新、切换页面、退出或清除后无法再次查看。</p>
      <p>手动保存到客户端的秘密环境配置 MCP_ACCESS_TOKEN，不要写入聊天、仓库或普通配置文件。不会自动复制到剪贴板。</p>
      <button aria-expanded={revealed} onClick={() => setRevealed(value => !value)}>{revealed ? "隐藏明文" : "显示一次性凭据"}</button>
      {revealed && <textarea aria-label="凭据明文" readOnly value={issued.token} spellCheck={false} rows={3} />}
      <button onClick={() => { clearSecret(); setNotice("本页明文已清除，无法重新查看。"); }}>清除本页明文</button>
    </div>}
    <div className="mcpListHeader"><h4>已授权的宿主</h4><button disabled={busy || expired} onClick={refresh}>刷新凭据列表</button></div>
    <small>最多显示 100 条，有效凭据优先。网页退出不撤销授权；重设密码会撤销全部凭据。刷新列表会清除本页明文。</small>
    {!loaded && !error && <p>正在读取凭据…</p>}
    {loaded && items.length === 0 && !expired && <p>尚无宿主授权。</p>}
    <ul className="mcpList">{items.map(item => <li key={item.id}>
      <h4>{item.host_name}</h4>
      <p>权限：只读知识检索 · {item.revoked_at ? "已撤销" : date(item.expires_at).getTime() <= now ? "已到期" : "有效"}</p>
      <small>签发：{time(item.created_at)}<br />到期：{time(item.expires_at)}</small>
      {!item.revoked_at && <button disabled={busy || expired || !!revokeId} onClick={() => { setRevokeId(item.id); setRevokeUnknown(false); }}>撤销 {item.host_name}</button>}
      {revokeId === item.id && <div className="mcpBoundary" role="region" aria-label="确认撤销凭据">
        <p>撤销后，此宿主不能发起新检索。已开始的调用仍可能完成。</p>
        <button disabled={busy || expired} onClick={() => void revoke()}>{revokeUnknown ? "重试撤销同一凭据" : "确认撤销"}</button>
        {!revokeUnknown && <button disabled={busy} onClick={() => setRevokeId(null)}>保留凭据</button>}
      </div>}
    </li>)}</ul>
  </section>;
}
