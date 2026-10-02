"use client";

import { useEffect, useRef, useState, type FormEvent } from "react";

type Hit = { url: string; title: string; snippet: string; text_truncated: boolean };
type Output = { provider: "searxng"; untrusted: true; partial: boolean; results_truncated: boolean; results: Hit[] };

function validOutput(value: unknown): value is Output {
  if (!value || typeof value !== "object") return false;
  const output = value as Output;
  return output.provider === "searxng" && output.untrusted === true
    && typeof output.partial === "boolean" && typeof output.results_truncated === "boolean"
    && Array.isArray(output.results) && output.results.length <= 5 && output.results.every(hit => {
      if (!hit || typeof hit.url !== "string" || hit.url.length > 2048
        || typeof hit.title !== "string" || Array.from(hit.title).length > 200
        || typeof hit.snippet !== "string" || Array.from(hit.snippet).length > 1000
        || typeof hit.text_truncated !== "boolean") return false;
      try {
        const url = new URL(hit.url);
        return ["http:", "https:"].includes(url.protocol) && !!url.hostname && !url.username && !url.password;
      } catch { return false; }
    });
}

function failure(status: number) {
  if (status === 401) return "登录已失效，请退出后重新登录。";
  if (status === 403) return "请求校验失败，请刷新页面后重试。";
  if (status === 404) return "管理员尚未启用外部搜索。";
  if (status === 409) return "该请求已使用，搜索不会再次执行，也不能恢复结果正文。";
  if (status === 429) return "工具调用额度已用完或服务繁忙，请稍后再试。";
  if (status === 400 || status === 422) return "查询格式无效，请输入 1–500 个字符且不含控制字符。";
  return "搜索服务暂不可用，不代表没有结果。本次尝试可能已计费，未自动重试。";
}

export function WebSearchPanel() {
  const [available, setAvailable] = useState<boolean | null>(null);
  const [expired, setExpired] = useState(false);
  const [query, setQuery] = useState("");
  const [consent, setConsent] = useState(false);
  const [busy, setBusy] = useState(true);
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [result, setResult] = useState<{ query: string; output: Output } | null>(null);
  const [requestId, setRequestId] = useState("");
  const pending = useRef<AbortController | null>(null);

  function expire() {
    setExpired(true); setQuery(""); setConsent(false); setResult(null); setRequestId(""); setNotice("");
  }

  useEffect(() => {
    const controller = new AbortController(); pending.current = controller;
    async function discover() {
      try {
        const response = await fetch("/api/tools", { cache: "no-store", signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) });
        if (pending.current !== controller) return;
        if (response.status === 401) expire();
        if (!response.ok) throw new Error(response.status === 401 || response.status === 403
          ? failure(response.status) : "无法读取搜索服务状态，请手动刷新。");
        const data = await response.json();
        if (pending.current !== controller) return;
        if (!Array.isArray(data?.tools) || !data.tools.every((tool: unknown) => tool && typeof tool === "object" && typeof (tool as { name?: unknown }).name === "string")) throw new Error("无法确认搜索服务状态，请手动刷新。");
        setAvailable(data.tools.some((tool: { name: string }) => tool.name === "web_search"));
      } catch (e) {
        if (pending.current === controller) setError(e instanceof Error && e.name === "Error" ? e.message : "无法读取搜索服务状态，请手动刷新。");
      } finally {
        if (pending.current === controller) { pending.current = null; setBusy(false); }
      }
    }
    void discover();
    return () => { pending.current?.abort(); pending.current = null; };
  }, [revision]);

  function refresh() {
    if (pending.current || expired) return;
    setBusy(true); setError(""); setAvailable(null); setConsent(false);
    setRevision(value => value + 1);
  }

  async function search(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (pending.current || !available || expired || !consent) return;
    const question = query.trim();
    if (!question || Array.from(question).length > 500 || /\p{Cc}/u.test(query)) {
      setError("查询格式无效，请输入 1–500 个字符且不含控制字符。"); return;
    }
    const controller = new AbortController(); pending.current = controller;
    setBusy(true); setError(""); setNotice(""); setResult(null); setConsent(false);
    const id = crypto.randomUUID(); setRequestId(id);
    try {
      const response = await fetch("/api/tools/web_search", {
        method: "POST", cache: "no-store",
        headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai", "Idempotency-Key": id },
        body: JSON.stringify({ query: question, limit: 5, acknowledge_external_request: true }),
        signal: AbortSignal.any([controller.signal, AbortSignal.timeout(25000)]),
      });
      if (pending.current !== controller) return;
      if (response.status === 401) expire();
      if (response.status === 404) setAvailable(false);
      if (!response.ok) throw new Error(failure(response.status));
      const data = await response.json();
      if (pending.current !== controller) return;
      if (data?.tool !== "web_search" || !validOutput(data.output)) throw new Error("服务返回无效搜索结果，未展示内容。本次尝试可能已计费，未自动重试。");
      setResult({ query: question, output: data.output });
    } catch (e) {
      if (pending.current === controller) setError(e instanceof Error && e.name === "Error" ? e.message : "网络异常或等待超时，结果未确认；服务端可能已执行并计费，未自动重试。");
    } finally {
      if (pending.current === controller) { pending.current = null; setBusy(false); }
    }
  }

  function stop() {
    pending.current?.abort(); pending.current = null; setBusy(false);
    setNotice("已停止等待。服务端可能仍在执行并计费；再次搜索需要重新确认，属于新的调用。");
  }

  return <section className="retrievalPanel webSearchPanel" aria-label="外部搜索">
    <h2>外部搜索</h2>
    <p>向管理员配置的搜索服务及其搜索引擎发送查询，可能产生服务费用。不会自动打开结果链接。</p>
    <button type="button" onClick={refresh} disabled={busy || expired}>刷新搜索状态</button>
    {available === false && <p role="status">管理员尚未启用外部搜索。</p>}
    {available === null && !error && <p role="status">正在读取搜索服务状态…</p>}
    {available && <form onSubmit={event => void search(event)}>
      <label htmlFor="web-search-query">搜索内容（1–500 个字符）</label>
      <textarea id="web-search-query" rows={3} value={query} disabled={busy || expired} required onChange={event => { setQuery(event.target.value); setConsent(false); }} />
      <label><input type="checkbox" checked={consent} disabled={busy || expired} onChange={event => setConsent(event.target.checked)} />我同意将本次查询发送给外部搜索服务及其搜索引擎，并承担可能的服务费用</label>
      <div className="retrievalActions">
        <button type="submit" disabled={busy || expired || !consent}>搜索公开资料</button>
        {busy && <button type="button" onClick={stop}>停止等待搜索</button>}
      </div>
    </form>}
    {notice && <p role="status">{notice}</p>}
    {error && <p role="alert">{error}</p>}
    {result && <section aria-label="外部搜索结果" aria-live="polite">
      <h3>本次查询：{result.query}</h3>
      <p>结果来自外部服务，未经知识库核验，请自行核对来源。</p>
      {result.output.partial && <p>部分搜索引擎未响应，以下结果可能不完整。</p>}
      {result.output.results_truncated && <p>仅展示前 5 条结果。</p>}
      {result.output.results.length ? <ol>{result.output.results.map((hit, index) => <li key={`${index}-${hit.url}`}>
        <h4><a href={hit.url} target="_blank" rel="noopener noreferrer" referrerPolicy="no-referrer">{hit.title || "未提供标题"}</a></h4>
        <small>{hit.url}</small><p className="answerText">{hit.snippet}</p>
        {hit.text_truncated && <small>标题或摘要已截断。</small>}
      </li>)}</ol> : <p>本次未返回匹配结果，不代表网上没有相关资料。</p>}
    </section>}
    {requestId && <p><small>本次请求 ID：{requestId}</small></p>}
    <small>不保存搜索历史，刷新、退出或切换账户后清空。不会自动重试；再次确认搜索会产生新的调用。</small>
  </section>;
}
