"use client";

import { useEffect, useState } from "react";

type Overview = {
  generated_at_unix_ms: number;
  knowledge: { total_documents: number; total_chunks: number; imported_today: number };
};

export function OverviewPanel({ revision }: { revision: number }) {
  const [data, setData] = useState<Overview | null>(null);
  const [error, setError] = useState("");
  const [refresh, setRefresh] = useState(0);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    const controller = new AbortController();
    async function load() {
      setLoading(true); setData(null); setError("");
      try {
        const response = await fetch("/api/overview", { cache: "no-store", signal: controller.signal });
        if (!response.ok) throw new Error(response.status === 401
          ? "登录已失效，请退出后重新登录。" : "概览暂不可用，请重试。");
        const result: Overview = await response.json();
        if (!controller.signal.aborted) setData(result);
      } catch (e) {
        if (!controller.signal.aborted) setError(e instanceof Error ? e.message : "概览加载失败。");
      } finally { if (!controller.signal.aborted) setLoading(false); }
    }
    void load();
    return () => controller.abort();
  }, [revision, refresh]);

  return <section className="overviewPanel" aria-label="今日概览" aria-busy={loading}>
    <h2>今日概览</h2>
    <p>今日导入按 UTC 00:00–24:00 统计（北京时间 08:00 至次日 08:00）。</p>
    {loading && <p role="status">正在加载概览…</p>}
    {error && <p role="alert">{error}</p>}
    {data && <>
      <dl className="overviewMetrics">
        <div><dt>文档总数</dt><dd>{data.knowledge.total_documents}</dd></div>
        <div><dt>文本块总数</dt><dd>{data.knowledge.total_chunks}</dd></div>
        <div><dt>今日导入</dt><dd>{data.knowledge.imported_today}</dd></div>
      </dl>
      {data.knowledge.total_documents === 0 && <p>还没有文档，导入第一份 Markdown 开始积累。</p>}
      <p>更新于 {new Date(data.generated_at_unix_ms).toLocaleString("zh-CN")}</p>
    </>}
    <button disabled={loading} onClick={() => setRefresh(value => value + 1)}>刷新概览</button>
  </section>;
}
