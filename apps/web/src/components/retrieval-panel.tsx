"use client";

import { useEffect, useRef, useState, type FormEvent } from "react";

type Hit = { document_id: string; ordinal: number; title: string; source: string; text: string; score: number };
type Citation = Hit & { id: number };
type Result = { query: string } & (
  | { kind: "search"; hits: Hit[] }
  | { kind: "answer"; answer: string; citations: Citation[] }
  | { kind: "insufficient" }
);

function isHit(value: unknown): value is Hit {
  if (!value || typeof value !== "object") return false;
  const hit = value as Hit;
  return [hit.document_id, hit.title, hit.source, hit.text].every(item => typeof item === "string")
    && Number.isInteger(hit.ordinal) && hit.ordinal >= 0 && Number.isFinite(hit.score);
}

function failure(status: number, code?: string) {
  if (status === 401) return "登录已失效，请退出后重新登录。";
  if (status === 403) return "请求校验失败，请刷新页面后重试。";
  if (code === "indexing_disabled") return "管理员尚未启用索引与检索。";
  if (code === "answering_disabled") return "管理员尚未启用知识问答，可先尝试检索资料。";
  if (status === 429) return "服务繁忙或模型限流，请稍后手动重试。";
  if (status === 400 || status === 422) return "请输入 1–1000 个字符的问题。";
  if (status === 504) return "处理超时，未自动重试；再次提交可能产生额外费用。";
  if (code === "invalid_answer") return "模型回答未通过引用校验，未展示该答案，请稍后重试。";
  return "检索或模型服务暂不可用；这不代表没有相关资料，请稍后重试。";
}

function Evidence({ hit, id }: { hit: Hit; id?: number }) {
  return <li>
    <h4>{id === undefined ? "" : `[${id}] `}{hit.title}</h4>
    <p>来源：{hit.source || "未提供"} · 第 {hit.ordinal + 1} 块 · 检索得分 {hit.score.toFixed(3)}</p>
    <details><summary>查看原文片段</summary><pre>{hit.text}</pre><small>文档 ID：{hit.document_id}</small></details>
  </li>;
}

export function RetrievalPanel() {
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [result, setResult] = useState<Result | null>(null);
  const pending = useRef<AbortController | null>(null);

  useEffect(() => () => { pending.current?.abort(); pending.current = null; }, []);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (pending.current) return;
    const question = query.trim();
    setError(""); setNotice(""); setResult(null);
    if (!question || Array.from(question).length > 1000) { setError("请输入 1–1000 个字符的问题。"); return; }
    const button = (event.nativeEvent as SubmitEvent).submitter as HTMLButtonElement | null;
    const mode = button?.value === "answer" ? "answer" : "search";
    const controller = new AbortController();
    pending.current = controller;
    setBusy(true);
    try {
      const response = await fetch(`/api/knowledge/${mode}`, {
        method: "POST", cache: "no-store",
        headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" },
        body: JSON.stringify(mode === "search" ? { query: question, limit: 5 } : { query: question }),
        signal: AbortSignal.any([controller.signal, AbortSignal.timeout(65000)]),
      });
      const data = await response.json().catch(() => null);
      if (pending.current !== controller) return;
      if (!response.ok) throw new Error(failure(response.status, data?.error?.code));
      // 防止无效成功响应被误展示成答案或「没有资料」。
      if (mode === "search" && Array.isArray(data?.hits) && data.hits.length <= 5 && data.hits.every(isHit)) {
        setResult({ kind: "search", query: question, hits: data.hits });
      } else if (mode === "answer" && data?.status === "insufficient_evidence" && data.answer === null && Array.isArray(data.citations) && data.citations.length === 0) {
        setResult({ kind: "insufficient", query: question });
      } else if (mode === "answer" && data?.status === "answered" && typeof data.answer === "string" && data.answer.trim()
        && Array.from(data.answer).length <= 4000 && Array.isArray(data.citations) && data.citations.length > 0 && data.citations.length <= 5
        && data.citations.every((hit: unknown) => isHit(hit) && Number.isInteger((hit as Citation).id) && (hit as Citation).id > 0)
        && new Set(data.citations.map((hit: Citation) => hit.id)).size === data.citations.length) {
        setResult({ kind: "answer", query: question, answer: data.answer, citations: data.citations });
      } else {
        throw new Error("服务返回无效结果，未展示答案或资料，请稍后重试。");
      }
    } catch (e) {
      if (pending.current === controller) setError(e instanceof Error && e.name === "Error" ? e.message : "网络异常或等待超时，未自动重试；再次提交可能产生额外费用。");
    } finally {
      if (pending.current === controller) { pending.current = null; setBusy(false); }
    }
  }

  function cancel() {
    pending.current?.abort(); pending.current = null; setBusy(false);
    setNotice("已停止等待。服务端可能仍在处理并计费，请勿立即重复提交。");
  }

  return <section className="retrievalPanel" aria-label="知识检索与问答">
    <h2>知识检索与问答</h2>
    <p>请先为文档建立索引；进行中的索引可能只返回部分资料。检索会调用向量模型，问答还可能调用聊天模型并产生费用，不自动重试。</p>
    <form onSubmit={event => void submit(event)}>
      <label htmlFor="knowledge-query">问题或检索内容（1–1000 个字符）</label>
      <textarea id="knowledge-query" value={query} onChange={event => setQuery(event.target.value)} disabled={busy} rows={3} required />
      <div className="retrievalActions">
        <button type="submit" value="search" disabled={busy}>检索资料</button>
        <button type="submit" value="answer" disabled={busy}>生成引用答案</button>
        {busy && <button type="button" onClick={cancel}>停止等待</button>}
      </div>
    </form>
    {busy && <p role="status">正在处理，请稍候…</p>}
    {notice && <p role="status">{notice}</p>}
    {error && <p role="alert">{error}</p>}
    {result && <section aria-label="本次查询结果" aria-live="polite">
      <h3>本次问题：{result.query}</h3>
      {result.kind === "search" && (result.hits.length ? <><p>找到 {result.hits.length} 个核验片段。检索得分不是正确率或置信概率。</p><ol>{result.hits.map(hit => <Evidence key={`${hit.document_id}:${hit.ordinal}`} hit={hit} />)}</ol></> : <p>未找到匹配的已索引片段；不代表整个知识库没有相关资料，请检查索引进度或调整问题。</p>)}
      {result.kind === "insufficient" && <p>证据不足，暂时无法回答。不代表整个知识库没有相关资料，请检查索引进度或调整问题。</p>}
      {result.kind === "answer" && <>
        <h4>答案</h4><p className="answerText">{result.answer}</p>
        <p>引用已核验归属，但不保证答案语义正确。请展开原文核对；检索得分不是置信概率。</p>
        <ol>{result.citations.map(hit => <Evidence key={hit.id} hit={hit} id={hit.id} />)}</ol>
      </>}
    </section>}
    <small>本页不保存问答历史，刷新、退出或切换账户后清空。答案与来源仅按文本展示。</small>
  </section>;
}
