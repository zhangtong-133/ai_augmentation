"use client";
import { useEffect, useState, type FormEvent } from "react";

type Summary = { id: string; title: string; source: string; tags: string[]; chunk_count: number; created_at_unix_ms: number };
type Document = Summary & { markdown: string; chunks: string[] };

function message(status: number) {
  if (status === 401) return "登录已失效，请退出后重新登录。";
  if (status === 409) return "这份内容已经导入，无需重复上传。";
  if (status === 413) return "文件太大，请选择不超过 256 KiB 的 Markdown。";
  if (status === 400 || status === 422) return "文档格式无效，请检查标题、正文及标签。";
  return "暂时无法访问知识库，请稍后重试。";
}

export function KnowledgePanel({ onImported }: { onImported: () => void }) {
  const [items, setItems] = useState<Summary[]>([]);
  const [selected, setSelected] = useState<Document | null>(null);
  const [offset, setOffset] = useState(0);
  const [revision, setRevision] = useState(0);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  useEffect(() => {
    const controller = new AbortController();
    async function load() {
      setLoading(true);
      try {
        const response = await fetch("/api/documents?offset=" + offset, { cache: "no-store", signal: controller.signal });
        if (!response.ok) throw new Error(message(response.status));
        setItems(await response.json());
      } catch (e) {
        if (!controller.signal.aborted) { setItems([]); setSelected(null); setError(e instanceof Error ? e.message : "加载失败"); }
      } finally { if (!controller.signal.aborted) setLoading(false); }
    }
    void load();
    return () => controller.abort();
  }, [offset, revision]);

  async function upload(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    setBusy(true); setError(""); setNotice("");
    try {
      const file = data.get("file");
      if (!(file instanceof File) || !/\.(md|markdown)$/i.test(file.name)) throw new Error("请选择 .md 或 .markdown 文件。");
      if (file.size > 256 * 1024) throw new Error(message(413));
      let markdown: string;
      try { markdown = new TextDecoder("utf-8", { fatal: true }).decode(await file.arrayBuffer()); }
      catch { throw new Error("请将文件保存为 UTF-8 编码后重新上传。"); }
      const tags = String(data.get("tags") ?? "").split(",").map(t => t.trim()).filter(Boolean);
      const title = String(data.get("title") ?? "").trim() || file.name;
      const response = await fetch("/api/documents", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" },
        body: JSON.stringify({ title, markdown, source: file.name, tags }),
      });
      if (!response.ok) throw new Error(message(response.status));
      const result: Summary = await response.json();
      setNotice("已导入「" + result.title + "」，生成 " + result.chunk_count + " 个文本块。");
      form.reset(); setOffset(0); setRevision(value => value + 1);
      onImported();
    } catch (e) { setError(e instanceof Error ? e.message : "导入失败"); }
    finally { setBusy(false); }
  }

  async function open(id: string) {
    setBusy(true); setError(""); setSelected(null);
    try {
      const response = await fetch("/api/documents/" + id, { cache: "no-store" });
      if (!response.ok) throw new Error(message(response.status));
      setSelected(await response.json());
    } catch (e) { setError(e instanceof Error ? e.message : "读取失败"); }
    finally { setBusy(false); }
  }

  return <section className="knowledgePanel" aria-label="个人知识库">
    <h2>个人知识库</h2>
    <p>导入 Markdown 笔记，保存原文并提取文本。知识问答尚未启用。</p>
    <form onSubmit={event => void upload(event)}>
      <label htmlFor="document-file">Markdown 文件（UTF-8，最多 256 KiB）</label>
      <input id="document-file" name="file" type="file" accept=".md,.markdown,text/markdown" required disabled={busy} />
      <label htmlFor="document-title">标题（可选，默认文件名）</label>
      <input id="document-title" name="title" maxLength={200} disabled={busy} />
      <label htmlFor="document-tags">标签（英文逗号分隔，最多 20 个）</label>
      <input id="document-tags" name="tags" disabled={busy} />
      <button disabled={busy}>{busy ? "处理中…" : "导入文档"}</button>
    </form>
    {error && <p role="alert">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    {loading ? <p role="status">正在加载文档…</p> : items.length === 0 ? <p>暂无文档。</p> :
      <ul>{items.map(item => <li key={item.id}>
        <button disabled={busy} onClick={() => void open(item.id)}>{item.title}</button>
        <span> {item.chunk_count} 块 · {item.tags.join(" / ")} · {new Date(item.created_at_unix_ms).toLocaleDateString("zh-CN")}</span>
      </li>)}</ul>}
    <div className="pagination">
      <button disabled={busy || loading || offset === 0} onClick={() => setOffset(value => Math.max(0, value - 20))}>上一页</button>
      <span>第 {offset / 20 + 1} 页</span>
      <button disabled={busy || loading || items.length < 20 || offset >= 100000} onClick={() => setOffset(value => value + 20)}>下一页</button>
      <button disabled={busy || loading} onClick={() => { setError(""); setRevision(value => value + 1); }}>刷新</button>
    </div>
    {selected && <section aria-label="文档详情">
      <h3>{selected.title}</h3>
      <button onClick={() => setSelected(null)}>关闭详情</button>
      <h4>原文</h4><pre>{selected.markdown}</pre>
      <details><summary>查看 {selected.chunk_count} 个文本块</summary>
        {selected.chunks.map((text, index) => <pre key={index}>{text}</pre>)}
      </details>
    </section>}
  </section>;
}
