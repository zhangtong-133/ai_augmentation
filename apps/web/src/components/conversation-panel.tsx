"use client";

import { useEffect, useRef, useState, type FormEvent } from "react";
import { requestError, useIdempotentPost } from "./use-idempotent-post";

type Conversation = { id: string; title: string };
type Message = { id: string; sequence: number; content: string };
type Snapshot = { revision: number; messages: Message[] };

function PendingWarning({ busy, discard }: { busy: boolean; discard: () => void }) {
  const [confirm, setConfirm] = useState(false);
  return <div className="pendingRequest">
    <p>保留了原请求，重试不会重复创建。刷新整个页面、退出账户或放弃后将丢失本地请求 ID；请先核对服务端历史，避免重复提交。</p>
    {!confirm ? <button type="button" disabled={busy} onClick={() => setConfirm(true)}>放弃未确认请求</button> : <>
      <p>放弃不会撤销可能已完成的写入，确定继续？</p>
      <button type="button" disabled={busy} onClick={discard}>确认放弃请求</button>
      <button type="button" disabled={busy} onClick={() => setConfirm(false)}>继续保留请求</button>
    </>}
  </div>;
}

export function ConversationPanel() {
  const [items, setItems] = useState<Conversation[]>([]);
  const [selected, setSelected] = useState<Conversation | null>(null);
  const [title, setTitle] = useState("");
  const [revision, setRevision] = useState(0);
  const [loading, setLoading] = useState(true);
  const [readError, setReadError] = useState("");
  const [inputError, setInputError] = useState("");
  const [threadLocked, setThreadLocked] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteBusy, setDeleteBusy] = useState(false);
  const [deleteError, setDeleteError] = useState("");
  const deletion = useRef<AbortController | null>(null);
  const create = useIdempotentPost<Conversation>("/api/conversations", value => {
    setTitle(""); setSelected(value); setItems(current => [value, ...current.filter(item => item.id !== value.id)]);
    setRevision(value => value + 1); setDeleting(false); setDeleteError("");
  });
  const locked = create.busy || !!create.pending || threadLocked || deleteBusy;
  useEffect(() => () => { deletion.current?.abort(); deletion.current = null; }, []);
  useEffect(() => {
    const controller = new AbortController();
    async function load() {
      setLoading(true); setReadError("");
      try {
        const response = await fetch("/api/conversations", { cache: "no-store", signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) });
        if (!response.ok) throw new Error(requestError(response.status));
        const data: Conversation[] = await response.json();
        if (controller.signal.aborted) return;
        setItems(data); setSelected(current => current && data.some(item => item.id === current.id) ? current : null);
      } catch (e) { if (!controller.signal.aborted) setReadError(e instanceof Error && e.name === "Error" ? e.message : "无法读取对话列表，请刷新重试。"); }
      finally { if (!controller.signal.aborted) setLoading(false); }
    }
    void load(); return () => controller.abort();
  }, [revision]);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); setInputError("");
    if (!title.trim() || [...title].length > 80 || title.includes("\0")) { setInputError("对话标题需 1–80 字，不能全为空白或含 NUL。"); return; }
    void create.submit({ title });
  }
  async function remove() {
    if (!selected || deletion.current) return;
    const active = new AbortController(); deletion.current = active; setDeleteBusy(true); setDeleteError("");
    try {
      const response = await fetch(`/api/conversations/${selected.id}`, { method: "DELETE", headers: { "X-Requested-With": "personal-ai" }, signal: AbortSignal.any([active.signal, AbortSignal.timeout(10000)]) });
      if (deletion.current !== active) return;
      if (response.status !== 404 && !response.ok) throw new Error(requestError(response.status));
      setItems(current => current.filter(item => item.id !== selected.id)); setSelected(null); setDeleting(false); setRevision(value => value + 1);
    } catch (e) { if (deletion.current === active) setDeleteError(e instanceof Error && e.name === "Error" ? e.message : "删除结果未确认，可重试删除或刷新列表核对。"); }
    finally { if (deletion.current === active) { deletion.current = null; setDeleteBusy(false); } }
  }

  return <section className="conversationPanel" aria-label="对话与消息">
    <h2>对话与消息</h2>
    <p>仅保存你发送的用户消息，不生成模型回复。消息持久保存至删除对话，每个对话最多 100 条；请勿保存敏感凭据。</p>
    <p>切换对话或退出会清空草稿。每账户最多 100 个对话，24 小时最多创建 100 个。</p>
    <form onSubmit={submit}>
      <label htmlFor="conversation-title">新对话标题（1–80 字）</label>
      <input id="conversation-title" value={title} disabled={locked} onChange={event => setTitle(event.target.value)} required />
      <button disabled={locked || loading}>创建对话</button>
    </form>
    {inputError && <p role="alert">{inputError}</p>}
    {create.error && <p role="alert">{create.error}</p>}
    {create.pending && !create.busy && <>
      <button disabled={threadLocked || deleteBusy} onClick={() => void create.submit({ title })}>重试创建原请求</button>
      <PendingWarning busy={create.busy} discard={create.discard} />
    </>}
    {create.busy && <p role="status">正在创建对话…</p>}
    <button disabled={loading || deleteBusy || create.busy || threadLocked} onClick={() => setRevision(value => value + 1)}>刷新对话列表</button>
    {readError && <p role="alert">{readError}</p>}
    {loading ? <p role="status">正在读取对话…</p> : !readError && items.length === 0 ? <p>暂无对话。</p> : <ul className="conversationList">{items.map(item => <li key={item.id}>
      <button disabled={locked} aria-pressed={selected?.id === item.id} onClick={() => { setSelected(item); setDeleting(false); setDeleteError(""); }}>{item.title}</button>
    </li>)}</ul>}
    {selected && <>
      <MessageThread key={selected.id} conversation={selected} onLock={setThreadLocked} disabled={create.busy || !!create.pending || deleteBusy || deleting} />
      <button disabled={locked} onClick={() => setDeleting(true)}>删除当前对话</button>
      {deleting && <div className="pendingRequest"><p>确认删除整个对话及全部消息？页面无法撤销；缓存和备份可能按保留策略延迟清理。</p>
        <button disabled={locked} onClick={() => void remove()}>确认删除对话</button>
        <button disabled={deleteBusy} onClick={() => setDeleting(false)}>取消删除对话</button>
      </div>}
      {deleteError && <p role="alert">{deleteError}</p>}
    </>}
  </section>;
}

function MessageThread({ conversation, onLock, disabled }: { conversation: Conversation; onLock: (value: boolean) => void; disabled: boolean }) {
  const [content, setContent] = useState("");
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [revision, setRevision] = useState(0);
  const [loading, setLoading] = useState(true);
  const [readError, setReadError] = useState("");
  const [inputError, setInputError] = useState("");
  const post = useIdempotentPost<Message>(`/api/conversations/${conversation.id}/messages`, () => { setContent(""); setRevision(value => value + 1); });
  const bytes = new TextEncoder().encode(content).length;
  useEffect(() => { onLock(post.busy || !!post.pending); return () => onLock(false); }, [onLock, post.busy, post.pending]);
  useEffect(() => {
    const controller = new AbortController();
    async function load() {
      setLoading(true); setReadError("");
      try {
        const response = await fetch(`/api/conversations/${conversation.id}/messages`, { cache: "no-store", signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) });
        if (!response.ok) throw new Error(requestError(response.status));
        const data: Snapshot = await response.json();
        if (!controller.signal.aborted) setSnapshot(data);
      } catch (e) { if (!controller.signal.aborted) { setSnapshot(null); setReadError(e instanceof Error && e.name === "Error" ? e.message : "无法读取消息，请刷新重试。"); } }
      finally { if (!controller.signal.aborted) setLoading(false); }
    }
    void load(); return () => controller.abort();
  }, [conversation.id, revision]);
  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); setInputError("");
    if (!content.trim() || bytes > 4096 || content.includes("\0")) { setInputError("消息需 1–4096 UTF-8 字节，不能全为空白或含 NUL。"); return; }
    void post.submit({ content });
  }
  return <section className="messageThread" aria-label="当前对话消息">
    <h3>{conversation.title}</h3>
    <button disabled={loading || post.busy || disabled} onClick={() => setRevision(value => value + 1)}>刷新消息</button>
    {readError && <p role="alert">{readError}</p>}
    {loading ? <p role="status">正在读取消息…</p> : snapshot && <>
      <p>已保存 {snapshot.messages.length}/100 条用户消息</p>
      {snapshot.messages.length === 0 ? <p>暂无消息。</p> : <ol className="messageList">{snapshot.messages.map(message => <li key={message.id}><span>用户消息 {message.sequence}</span><p>{message.content}</p></li>)}</ol>}
    </>}
    <form onSubmit={submit}>
      <label htmlFor="conversation-message">用户消息（最多 4096 字节）</label>
      <textarea id="conversation-message" rows={4} value={content} onChange={event => setContent(event.target.value)} disabled={post.busy || !!post.pending || disabled} required />
      <p>{bytes}/4096 字节；仅保存，不会调用模型。</p>
      <button disabled={post.busy || !!post.pending || disabled || loading || !snapshot || snapshot.messages.length >= 100}>发送用户消息</button>
    </form>
    {inputError && <p role="alert">{inputError}</p>}
    {post.error && <p role="alert">{post.error}</p>}
    {post.busy && <p role="status">正在发送…</p>}
    {post.pending && !post.busy && <>
      <button disabled={disabled} onClick={() => void post.submit({ content })}>重试发送原请求</button>
      <PendingWarning busy={post.busy || disabled} discard={post.discard} />
    </>}
  </section>;
}
