"use client";

import { useEffect, useRef, useState } from "react";
import { executionStatuses, isGrant, statuses, type Citation, type Grant, type Source } from "./answer-authorization-types";
const base = "/api/knowledge/answer-authorizations";
function metadata(grant: Grant): Grant {
  return { ...grant, preview: null, execution: grant.execution ? { ...grant.execution, question: null, result: null } : null };
}
function failure(status: number, code?: string) {
  if (status === 401) return "登录已失效，请重新登录。";
  if (status === 403) return "请求校验失败，请刷新页面。";
  if (code === "answer_execution_disabled") return "管理员尚未启用本地问答执行，授权尚未消费。请查询原请求状态。";
  if (code === "answer_execution_target_changed") return "当前执行目标与已确认请求不同。请重新预览并确认当前目标。";
  if (status === 404) return "未找到该请求或来源，请查询原请求或重新检索。";
  if (status === 409) return "请求已失效、内容冲突或达到授权上限，请查询原请求状态。";
  if (status === 400 || status === 422) return "预览输入无效或材料超过本地请求预算，请减少选择。";
  return "授权服务暂不可用，请查询原请求状态。";
}
function ExecutionCitation({ citation }: { citation: Citation }) {
  const text = Array.from(citation.text);
  return <li>
    <h5>[{citation.id}] {citation.title}</h5>
    <p>来源：{citation.source || "未提供"} · 第 {citation.ordinal + 1} 块</p>
    <blockquote className="citationQuote">{citation.quote}</blockquote>
    <details><summary>查看引用原文与位置</summary><pre>{text.slice(0, citation.quote_start).join("")}<mark>{citation.quote}</mark>{text.slice(citation.quote_end).join("")}</pre>
      <small>文档 ID：{citation.document_id} · 字符位置 {citation.quote_start}–{citation.quote_end}</small></details>
  </li>;
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
  const [sending, setSending] = useState(false);
  const [error, setError] = useState("");
  const [attempted, setAttempted] = useState<Set<string>>(() => new Set());
  const pending = useRef<AbortController | null>(null);
  useEffect(() => () => { pending.current?.abort(); pending.current = null; }, []);

  async function run(path: string, id: string, payload?: unknown, listing = false, executing = false) {
    if (pending.current) return;
    const controller = new AbortController(); pending.current = controller;
    setBusy(true); setSending(executing); setError("");
    if (!executing) setGrant(null);
    setSharing(false); setCompute(false);
    if (id) setRecover(id);
    try {
      const response = await fetch(path, {
        method: payload === undefined ? "GET" : "POST", cache: "no-store",
        headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" },
        ...(payload === undefined ? {} : { body: JSON.stringify(payload) }),
        signal: AbortSignal.any([controller.signal, AbortSignal.timeout(20000)]),
      });
      const data: unknown = await response.json().catch(() => null);
      if (pending.current !== controller) return;
      if (!response.ok) {
        if (response.status === 401) { setItems([]); setRecover(""); }
        if ((path === base && [400, 422].includes(response.status)) || (payload === undefined && response.status === 404)) setRecover("");
        const problem = data as { error?: { code?: string } } | null;
        throw new Error(failure(response.status, problem?.error?.code));
      }
      if (listing) {
        const list = data as { items?: unknown; execution_available?: unknown };
        if (!Array.isArray(list?.items) || list.items.length > 20 || !list.items.every(item => isGrant(item, true))
          || typeof list.execution_available !== "boolean" || list.items.some(item => item.execution_available !== list.execution_available)) throw new Error("授权记录格式异常。");
        setItems(list.items);
      } else {
        if (!isGrant(data) || data.request_id !== id) throw new Error("授权预览格式异常，请查询原请求状态。");
        setGrant(data); setRecover("");
        setItems(previous => [metadata(data), ...previous.filter(a => a.request_id !== id)].slice(0, 20));
      }
    } catch (e) {
      if (pending.current === controller) {
        setGrant(null);
        setError(e instanceof Error && e.name === "Error" ? e.message : "网络异常，结果尚未确认。请查询原请求；系统不会自动重发。");
      }
    } finally {
      if (pending.current === controller) { pending.current = null; setBusy(false); setSending(false); }
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
  function execute() {
    if (!grant || grant.status !== "authorized" || !grant.execution_available || grant.execution || pending.current || attempted.has(grant.request_id)) return;
    if (Date.now() >= grant.expires_at_unix_ms) { setError("授权已过期，请查询原请求状态。"); return; }
    setAttempted(previous => new Set(previous).add(grant.request_id));
    void run(`${base}/${grant.request_id}/execute`, grant.request_id, { digest: grant.digest }, false, true);
  }
  function cancel() {
    if (!grant || pending.current && !sending) return;
    // 取消可以中断等待执行入口；晚到的响应必须失去当前控制器，不能恢复正文。
    pending.current?.abort(); pending.current = null;
    void run(`${base}/${grant.request_id}/cancel`, grant.request_id, {});
  }
  return <section className="answerAuthorizations" aria-label="本地问答授权">
    <h3>本地问答预览与授权</h3>
    <p>先预览并保存一次授权，再另行点击执行一次。保存、刷新或查询均不会发送模型请求。授权 10 分钟后失效；执行结果仍需核对。</p>
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
    {busy && <p role="status">{sending ? "正在提交本次执行，可取消；不会自动重发…" : "正在读取或保存授权…"}</p>}
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
      {grant.status === "authorized" && !grant.execution && <>
        {grant.execution_available ? <><p>执行将把刚确认的问题和材料发送给预览中的目标，并使用本地计算资源。每份授权只领取一次，失败或结果未知也不会重发。</p>
          <button type="button" disabled={busy || attempted.has(grant.request_id)} onClick={execute}>执行一次</button></>
          : <p>当前部署尚未启用本地问答执行；已保存的授权不会自动执行。</p>}
      </>}
      {grant.execution && <section aria-label="本次执行结果">
        <h4>{executionStatuses[grant.execution.status]}</h4>
        <p>此授权已领取，不会再次执行。刷新只读取持久化状态。</p>
        <p>开始时间：{new Date(grant.execution.started_at_unix_ms).toLocaleString()}</p>
        <button type="button" disabled={busy} onClick={() => void run(`${base}/${grant.request_id}`, grant.request_id)}>查询执行状态</button>
        {grant.execution.status === "succeeded" && grant.execution.result && <>
          {grant.execution.question !== null && <h5>本次问题：{grant.execution.question}</h5>}
          <p>这是模型结果，待你核对。摘录已与原文逐字校验，不能保证模型结论正确。</p>
          {grant.execution.result.status === "insufficient_evidence" ? <p>证据不足，模型未给出答案。不代表整个知识库没有相关资料。</p> : <>
            <h5>答案</h5><p className="answerText">{grant.execution.result.answer}</p>
            <ol>{grant.execution.result.citations.map(citation => <ExecutionCitation key={citation.id} citation={citation} />)}</ol>
          </>}
        </>}
      </section>}
      {grant.status === "consumed" && !grant.execution && <><p>此授权已领取，不能再次执行。请查询原请求核对状态。</p>
        <button type="button" disabled={busy} onClick={() => void run(`${base}/${grant.request_id}`, grant.request_id)}>查询执行状态</button></>}
      {(["draft", "authorized"].includes(grant.status) || grant.execution && ["running", "succeeded"].includes(grant.execution.status))
        && <button type="button" disabled={busy && !sending} onClick={cancel}>{grant.execution?.status === "succeeded" ? "删除本次结果" : "取消该授权"}</button>}
    </section>}
  </section>;
}
