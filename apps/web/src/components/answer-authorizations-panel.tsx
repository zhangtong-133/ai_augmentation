"use client";

import { useEffect, useRef, useState } from "react";

type Source = { document_id: string; ordinal: number; title: string; source: string; text: string };
type Grant = {
  request_id: string; status: string; digest: string; created_at_unix_ms: number; expires_at_unix_ms: number;
  preview: null | { question: string; materials: Source[]; request_sha256: string; request: { endpoint: string; body: unknown; profile: string } };
};
const statuses: Record<string, string> = { draft: "待确认", authorized: "已保存授权", cancelled: "已取消", expired: "已过期", invalidated: "来源已失效", consumed: "已领取" };
const base = "/api/knowledge/answer-authorizations";
function isGrant(value: unknown): value is Grant {
  if (!value || typeof value !== "object") return false;
  const a = value as Grant;
  if (typeof a.request_id !== "string" || !/^[0-9a-f-]{36}$/.test(a.request_id) || !Object.hasOwn(statuses, a.status)
    || typeof a.digest !== "string" || !/^[0-9a-f]{64}$/.test(a.digest)
    || !Number.isSafeInteger(a.created_at_unix_ms) || !Number.isSafeInteger(a.expires_at_unix_ms)) return false;
  if (a.preview === null) return true;
  const p = a.preview;
  return !!p && ["draft", "authorized"].includes(a.status) && typeof p.question === "string"
    && typeof p.request_sha256 === "string" && typeof p.request?.endpoint === "string" && typeof p.request.profile === "string"
    && !!p.request.body && typeof p.request.body === "object"
    && Array.isArray(p.materials) && p.materials.length > 0 && p.materials.length <= 5
    && p.materials.every(s => !!s && [s.document_id, s.title, s.source, s.text].every(v => typeof v === "string") && Number.isSafeInteger(s.ordinal) && s.ordinal >= 0);
}
function failure(status: number) {
  if (status === 401) return "登录已失效，请重新登录。";
  if (status === 403) return "请求校验失败，请刷新页面。";
  if (status === 404) return "未找到该请求或来源，请查询原请求或重新检索。";
  if (status === 409) return "请求已失效、内容冲突或达到授权上限，请查询原请求状态。";
  if (status === 400 || status === 422) return "预览输入无效或材料超过本地请求预算，请减少选择。";
  return "授权服务暂不可用，请查询原请求状态。";
}

export function AnswerAuthorizationsPanel({ query, sources }: { query: string; sources: Source[] }) {
  const [selected, setSelected] = useState<number[]>([]);
  const [endpoint, setEndpoint] = useState("http://127.0.0.1:11435");
  const [model, setModel] = useState("qwen3:4b-q4_K_M");
  const [items, setItems] = useState<Grant[]>([]);
  const [grant, setGrant] = useState<Grant | null>(null);
  const [recover, setRecover] = useState("");
  const [sharing, setSharing] = useState(false);
  const [compute, setCompute] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const pending = useRef<AbortController | null>(null);
  useEffect(() => () => { pending.current?.abort(); pending.current = null; }, []);

  async function run(path: string, id: string, payload?: unknown, listing = false) {
    if (pending.current) return;
    const controller = new AbortController(); pending.current = controller;
    setBusy(true); setError(""); setGrant(null); setSharing(false); setCompute(false);
    if (id) setRecover(id);
    try {
      const response = await fetch(path, {
        method: payload === undefined ? "GET" : "POST", cache: "no-store",
        headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" },
        ...(payload === undefined ? {} : { body: JSON.stringify(payload) }),
        signal: AbortSignal.any([controller.signal, AbortSignal.timeout(20000)]),
      });
      const data: unknown = await response.json();
      if (pending.current !== controller) return;
      if (!response.ok) {
        if ((path === base && [400, 422].includes(response.status)) || (payload === undefined && response.status === 404)) setRecover("");
        throw new Error(failure(response.status));
      }
      if (listing) {
        const list = data as { items?: unknown; execution_available?: unknown };
        if (!Array.isArray(list?.items) || list.items.length > 20 || !list.items.every(isGrant)
          || list.items.some(a => a.preview !== null) || list.execution_available !== false) throw new Error("授权记录格式异常。");
        setItems(list.items);
      } else {
        if (!isGrant(data) || data.request_id !== id) throw new Error("授权预览格式异常，请查询原请求状态。");
        setGrant(data); setRecover("");
        setItems(previous => [ { ...data, preview: null }, ...previous.filter(a => a.request_id !== id) ].slice(0, 20));
      }
    } catch (e) {
      if (pending.current === controller) setError(e instanceof Error && e.name === "Error" ? e.message : "网络异常，结果尚未确认。请查询原请求；系统不会自动重发。");
    } finally {
      if (pending.current === controller) { pending.current = null; setBusy(false); }
    }
  }
  function prepare() {
    const id = crypto.randomUUID();
    void run(base, id, { request_id: id, question: query, endpoint, model,
      sources: selected.map(index => ({ document_id: sources[index].document_id, ordinal: sources[index].ordinal })) });
  }
  function approve() {
    if (!grant?.preview || !sharing || !compute) return;
    if (Date.now() >= grant.expires_at_unix_ms) { setError("预览已过期，请查询原请求状态。"); setSharing(false); setCompute(false); return; }
    void run(`${base}/${grant.request_id}/approve`, grant.request_id, { digest: grant.digest, acknowledge_sharing: sharing, acknowledge_local_compute: compute });
  }
  return <section className="answerAuthorizations" aria-label="本地问答授权">
    <h3>本地问答预览与授权</h3>
    <p>仅保存精确预览与一次授权，当前尚未开放执行。此处不会启动模型、发送材料或再次向量化。授权 10 分钟后失效。</p>
    {sources.length > 0 ? <fieldset disabled={busy || !!recover}>
      <legend>选择本次问题的材料：{query}</legend>
      {sources.map((source, index) => <label key={`${source.document_id}:${source.ordinal}`} className="authorizationChoice">
        <input type="checkbox" checked={selected.includes(index)} onChange={event => setSelected(previous => event.target.checked ? [...previous, index] : previous.filter(i => i !== index))} />
        {source.title} · 第 {source.ordinal + 1} 块
      </label>)}
      <label>本地服务地址<input value={endpoint} onChange={e => setEndpoint(e.target.value)} /></label>
      <label>本地模型名称<input value={model} onChange={e => setModel(e.target.value)} /></label>
      <button type="button" onClick={prepare} disabled={!selected.length}>创建精确预览</button>
    </fieldset> : <p>先检索资料，再选择片段创建预览；已有授权可从下方恢复。</p>}
    <button type="button" disabled={busy} onClick={() => void run(base, "", undefined, true)}>刷新授权记录</button>
    {busy && <p role="status">正在读取或保存授权…</p>}
    {error && <p role="alert">{error}</p>}
    {recover && <p>待确认请求：<code>{recover}</code> <button type="button" disabled={busy} onClick={() => void run(`${base}/${recover}`, recover)}>查询原请求</button></p>}
    <ul aria-label="最近授权记录">{items.map(item => <li key={item.request_id}>
      <button type="button" disabled={busy} onClick={() => void run(`${base}/${item.request_id}`, item.request_id)}>{statuses[item.status]} · {item.request_id}</button>
    </li>)}</ul>
    {grant && <section aria-label="精确授权详情">
      <p role="status">状态：{statuses[grant.status]}。保存授权不会执行模型。</p>
      <p>请求：<code>{grant.request_id}</code></p>
      <p>有效期至：{new Date(grant.expires_at_unix_ms).toLocaleString()}</p>
      {grant.preview && <>
        <h4>授权问题：{grant.preview.question}</h4>
        <p>发送目标：{grant.preview.request.endpoint}</p>
        <ol>{grant.preview.materials.map(s => <li key={`${s.document_id}:${s.ordinal}`}><strong>{s.title}</strong><p>{s.source} · 第 {s.ordinal + 1} 块</p><pre>{s.text}</pre></li>)}</ol>
        <details><summary>查看完整模型请求与指纹</summary><pre>{JSON.stringify(grant.preview.request, null, 2)}</pre><p>请求指纹：<code>{grant.preview.request_sha256}</code></p><p>授权摘要：<code>{grant.digest}</code></p></details>
        {grant.status === "draft" && <fieldset disabled={busy}>
          <legend>确认当前显示的完整请求</legend>
          <label className="authorizationChoice"><input type="checkbox" checked={sharing} onChange={e => setSharing(e.target.checked)} />我同意将当前问题和所选原文分享给显示的本地目标与模型</label>
          <label className="authorizationChoice"><input type="checkbox" checked={compute} onChange={e => setCompute(e.target.checked)} />我理解未来执行会使用本地计算资源，本次只保存授权</label>
          <button type="button" disabled={!sharing || !compute} onClick={approve}>保存一次授权</button>
        </fieldset>}
      </>}
      {["draft", "authorized"].includes(grant.status) && <button type="button" disabled={busy} onClick={() => void run(`${base}/${grant.request_id}/cancel`, grant.request_id, {})}>取消该授权</button>}
    </section>}
  </section>;
}
