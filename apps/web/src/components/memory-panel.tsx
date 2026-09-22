"use client";

import { useEffect, useRef, useState, type FormEvent } from "react";

type Fact = { id: string; title: string; content: string; version: number };
function message(status: number) {
  if (status === 401) return "登录已失效，请退出后重新登录。";
  if (status === 409) return "记录已被修改或已达到 100 条上限，请刷新列表后重新操作。";
  if (status === 404) return "记录已不存在或无权访问，请刷新列表。";
  if (status === 400 || status === 422) return "标题需 1–80 字，内容需 1–2000 字，不能全为空白。";
  return "记忆服务暂不可用。写入结果可能未确认，请先刷新列表，不要直接重复提交。";
}

export function MemoryPanel() {
  const [facts, setFacts] = useState<Fact[]>([]);
  const [offset, setOffset] = useState(0);
  const [revision, setRevision] = useState(0);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [editing, setEditing] = useState<Fact | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");
  const pending = useRef<AbortController | null>(null);

  useEffect(() => () => { pending.current?.abort(); pending.current = null; }, []);
  useEffect(() => {
    const controller = new AbortController();
    async function load() {
      setLoading(true);
      try {
        const response = await fetch(`/api/memories?offset=${offset}`, { cache: "no-store", signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) });
        if (!response.ok) throw new Error(message(response.status));
        const data = await response.json();
        if (!controller.signal.aborted) { setFacts(data); setError(""); }
      } catch (e) {
        if (!controller.signal.aborted) { setFacts([]); setError(e instanceof Error && e.name === "Error" ? e.message : "无法读取记忆，请刷新重试。"); }
      } finally { if (!controller.signal.aborted) setLoading(false); }
    }
    void load();
    return () => controller.abort();
  }, [offset, revision]);

  function reset() { setEditing(null); setDeleting(null); setTitle(""); setContent(""); }
  async function mutate(method: "POST" | "PUT" | "DELETE", fact?: Fact) {
    if (pending.current) return;
    setError(""); setNotice("");
    if (method !== "DELETE" && (!title.trim() || !content.trim() || Array.from(title).length > 80 || Array.from(content).length > 2000)) { setError(message(400)); return; }
    const controller = new AbortController(); pending.current = controller; setBusy(true);
    try {
      const response = await fetch(`/api/memories${fact ? `/${fact.id}` : ""}`, {
        method, headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" },
        body: JSON.stringify(method === "DELETE" ? { version: fact?.version } : { title, content, ...(fact ? { version: fact.version } : {}) }),
        signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]),
      });
      if (pending.current !== controller) return;
      if (!response.ok) throw new Error(message(response.status));
      reset(); setNotice(method === "DELETE" ? "记忆已删除。" : "记忆已保存。"); setOffset(0); setRevision(value => value + 1);
    } catch (e) {
      if (pending.current === controller) setError(e instanceof Error && e.name === "Error" ? e.message : message(503));
    } finally { if (pending.current === controller) { pending.current = null; setBusy(false); } }
  }
  function save(event: FormEvent<HTMLFormElement>) { event.preventDefault(); void mutate(editing ? "PUT" : "POST", editing ?? undefined); }

  return <section className="memoryPanel" aria-label="长期记忆">
    <h2>长期记忆</h2>
    <p>仅保存你手动确认的事实与偏好，每个账户最多 100 条。当前不会发送给模型或自动用于问答；请勿保存密码等敏感凭据。</p>
    <form onSubmit={save}>
      <h3>{editing ? "编辑记忆" : "新增记忆"}</h3>
      <label htmlFor="memory-title">记忆标题（1–80 字）</label>
      <input id="memory-title" value={title} onChange={event => setTitle(event.target.value)} required disabled={busy} />
      <label htmlFor="memory-content">记忆内容（1–2000 字）</label>
      <textarea id="memory-content" rows={3} value={content} onChange={event => setContent(event.target.value)} required disabled={busy} />
      <button disabled={busy || loading}>{busy ? "正在保存…" : "保存记忆"}</button>
      {editing && <button type="button" disabled={busy} onClick={reset}>取消编辑</button>}
    </form>
    {error && <p role="alert">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    {loading ? <p role="status">正在加载记忆…</p> : !error && facts.length === 0 ? <p>暂无记忆。</p> : <ul>{facts.map(fact => <li key={fact.id}>
      <h3>{fact.title}</h3><p className="memoryText">{fact.content}</p>
      <button disabled={busy} onClick={() => { setEditing(fact); setDeleting(null); setTitle(fact.title); setContent(fact.content); setError(""); setNotice(""); }}>编辑</button>
      <button disabled={busy} onClick={() => setDeleting(fact.id)}>删除</button>
      {deleting === fact.id && <div><p>确认永久删除这条记忆？此操作无法在页面撤销。</p><button disabled={busy} onClick={() => void mutate("DELETE", fact)}>确认删除</button><button disabled={busy} onClick={() => setDeleting(null)}>取消删除</button></div>}
    </li>)}</ul>}
    <div className="pagination">
      <button disabled={busy || loading || offset === 0} onClick={() => { reset(); setOffset(value => Math.max(0, value - 20)); }}>上一页记忆</button>
      <span>第 {offset / 20 + 1} 页</span>
      <button disabled={busy || loading || facts.length < 20 || offset >= 80} onClick={() => { reset(); setOffset(value => value + 20); }}>下一页记忆</button>
      <button disabled={busy || loading} onClick={() => { reset(); setNotice(""); setRevision(value => value + 1); }}>刷新记忆</button>
    </div>
  </section>;
}
