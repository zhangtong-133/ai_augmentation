"use client";
import { useEffect, useState, type FormEvent } from "react";

type Summary = { source_type: "markdown" | "pdf" | "web_page"; id: string; title: string; source: string; tags: string[]; chunk_count: number; created_at_unix_ms: number };
type Document = Summary & { markdown: string; chunks: string[] };

function message(status: number, code?: string) {
  if (code === "invalid_url") return "请输入不含登录信息的 HTTP 或 HTTPS 网页地址（使用默认端口）。";
  if (code === "url_blocked") return "仅支持公开网页，不能导入本机或内网地址。";
  if (code === "web_too_large") return "网页过大，请选择不超过 1 MiB 的 HTML 页面。";
  if (code === "web_unsupported") return "暂仅支持 UTF-8 HTML 网页，请将其他格式另存为文件后导入。";
  if (code === "web_empty") return "网页没有可提取正文；需要登录或运行脚本的页面暂不支持。";
  if (code === "web_timeout") return "网页抓取超时，请稍后重试。";
  if (code === "web_busy") return "网页导入繁忙，请稍后重试。";
  if (code === "web_redirect") return "网页重定向过多或无效，请使用最终页面地址。";
  if (code === "web_unavailable") return "无法获取网页，请检查地址或稍后重试。";
  if (code === "invalid_pdf") return "PDF 无法读取，请检查文件是否损坏或需要密码。";
  if (code === "pdf_no_text") return "PDF 中没有可提取文字；扫描件请先进行 OCR。";
  if (code === "pdf_timeout") return "PDF 解析超时，请拆分文件后重试。";
  if (code === "pdf_busy") return "PDF 导入繁忙，请稍后重试。";
  if (code === "pdf_unavailable") return "PDF 解析服务暂不可用，请稍后重试。";
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
      if (!(file instanceof File) || !/\.(md|markdown|pdf)$/i.test(file.name)) throw new Error("请选择 .md、.markdown 或 .pdf 文件。");
      const isPdf = /\.pdf$/i.test(file.name);
      if (file.size > (isPdf ? 5 * 1024 * 1024 : 256 * 1024)) {
        throw new Error(isPdf ? "PDF 太大，请选择不超过 5 MiB 的文件。" : message(413));
      }
      const buffer = await file.arrayBuffer();
      let content: { markdown: string } | { pdf_base64: string };
      if (isPdf) {
        const bytes = new Uint8Array(buffer);
        let binary = "";
        for (let index = 0; index < bytes.length; index += 8192) {
          binary += String.fromCharCode(...bytes.subarray(index, index + 8192));
        }
        content = { pdf_base64: btoa(binary) };
      } else {
        try { content = { markdown: new TextDecoder("utf-8", { fatal: true }).decode(buffer) }; }
        catch { throw new Error("请将文件保存为 UTF-8 编码后重新上传。"); }
      }
      const tags = String(data.get("tags") ?? "").split(",").map(t => t.trim()).filter(Boolean);
      const title = String(data.get("title") ?? "").trim() || file.name;
      const response = await fetch("/api/documents", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" },
        body: JSON.stringify({ title, ...content, source: file.name, tags }),
      });
      if (!response.ok) {
        const failure = await response.json().catch(() => null);
        if (isPdf && response.status === 413) throw new Error("PDF 或提取文本过大，请拆分文件后重试。");
        throw new Error(message(response.status, failure?.error?.code));
      }
      const result: Summary = await response.json();
      setNotice("已导入「" + result.title + "」，生成 " + result.chunk_count + " 个文本块。");
      form.reset(); setOffset(0); setRevision(value => value + 1);
      onImported();
    } catch (e) { setError(e instanceof Error ? e.message : "导入失败"); }
    finally { setBusy(false); }
  }

  async function importUrl(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    setBusy(true); setError(""); setNotice("");
    try {
      const response = await fetch("/api/documents", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" },
        body: JSON.stringify({ url: String(data.get("url") ?? "").trim(), title: String(data.get("title") ?? "").trim(),
          tags: String(data.get("tags") ?? "").split(",").map(tag => tag.trim()).filter(Boolean) }),
      });
      if (!response.ok) {
        const failure = await response.json().catch(() => null);
        throw new Error(message(response.status, failure?.error?.code));
      }
      const result: Summary = await response.json();
      setNotice("已导入「" + result.title + "」，生成 " + result.chunk_count + " 个文本块。");
      form.reset(); setOffset(0); setRevision(value => value + 1); onImported();
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
    <p>导入 Markdown 或 PDF，保存原文件并提取文本。扫描 PDF 请先进行 OCR；知识问答尚未启用。</p>
    <form onSubmit={event => void upload(event)}>
      <label htmlFor="document-file">Markdown / PDF 文件（Markdown 为 UTF-8，最多 256 KiB；PDF 最多 5 MiB）</label>
      <input id="document-file" name="file" type="file" accept=".md,.markdown,.pdf,text/markdown,application/pdf" required disabled={busy} />
      <label htmlFor="document-title">标题（可选，默认文件名）</label>
      <input id="document-title" name="title" maxLength={200} disabled={busy} />
      <label htmlFor="document-tags">标签（英文逗号分隔，最多 20 个）</label>
      <input id="document-tags" name="tags" disabled={busy} />
      <button disabled={busy}>{busy ? "处理中…" : "导入文档"}</button>
    </form>
    <h3>导入网页</h3>
    <p>支持公开的 UTF-8 静态网页（HTML 最多 1 MiB）。登录页面和需要运行脚本的内容暂不支持。</p>
    <form onSubmit={event => void importUrl(event)}>
      <label htmlFor="document-url">网页地址</label>
      <input id="document-url" name="url" type="url" maxLength={2048} placeholder="https://example.com/article" required disabled={busy} />
      <label htmlFor="web-title">网页标题（可选，默认页面标题）</label>
      <input id="web-title" name="title" maxLength={200} disabled={busy} />
      <label htmlFor="web-tags">网页标签（英文逗号分隔，最多 20 个）</label>
      <input id="web-tags" name="tags" disabled={busy} />
      <button disabled={busy}>{busy ? "处理中…" : "导入网页"}</button>
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
      {selected.source_type === "web_page" && <p>来源：<a href={selected.source} target="_blank" rel="noopener noreferrer">{selected.source}</a></p>}
      <h4>{selected.source_type === "pdf" ? "PDF 提取文本" : selected.source_type === "web_page" ? "网页提取文本" : "原文"}</h4><pre>{selected.markdown}</pre>
      <details><summary>查看 {selected.chunk_count} 个文本块</summary>
        {selected.chunks.map((text, index) => <pre key={index}>{text}</pre>)}
      </details>
    </section>}
  </section>;
}
