"use client";

import { useEffect, useRef, useState } from "react";

type Job = {
  document_id: string;
  status: "queued" | "running" | "retrying" | "completed" | "failed";
  indexed_chunks: number;
  total_chunks: number;
  attempts: number;
  error_code: string | null;
};

const labels = { queued: "等待索引", running: "正在索引", retrying: "等待自动重试", completed: "索引完成", failed: "索引失败" };
const failures: Record<string, string> = {
  embedding_unavailable: "向量模型暂不可用",
  embedding_invalid: "向量模型返回无效结果",
  vector_store_unavailable: "向量存储暂不可用",
  document_unavailable: "文档暂不可用",
  invalid_progress: "索引进度异常",
  indexing_timeout: "索引处理超时",
};

export function IndexJobPanel({ documentId, title }: { documentId: string; title: string }) {
  const [job, setJob] = useState<Job | null>(null);
  const [state, setState] = useState<"loading" | "ready" | "disabled" | "error">("loading");
  const [error, setError] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const request = useRef<AbortController | null>(null);
  const active = job && ["queued", "running", "retrying"].includes(job.status);

  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function load() {
      const controller = new AbortController();
      request.current = controller;
      try {
        const response = await fetch(`/api/documents/${documentId}/index-job`, {
          cache: "no-store", signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]),
        });
        const data = await response.json();
        if (stopped) return;
        if (response.status === 404) { setJob(null); setState("ready"); setError(""); return; }
        if (data?.error?.code === "indexing_disabled") { setJob(null); setState("disabled"); setError(""); return; }
        if (!response.ok) throw new Error(response.status === 401 ? "登录已失效，请重新登录。" : "无法读取索引状态，请刷新重试。");
        setJob(data); setState("ready"); setError("");
        // 只轮询未结束任务；失败停止自动请求，不自动提交付费任务。
        if (["queued", "running", "retrying"].includes(data.status)) timer = setTimeout(() => void load(), 2000);
      } catch (e) {
        if (!stopped && !controller.signal.aborted) { setState("error"); setError(e instanceof Error && e.name === "Error" ? e.message : "无法读取索引状态，请刷新重试。"); }
      }
    }
    void load();
    return () => { stopped = true; clearTimeout(timer); request.current?.abort(); };
  }, [documentId, refresh]);

  async function submit() {
    if (submitting) return;
    setSubmitting(true); setError("");
    request.current?.abort();
    const controller = new AbortController();
    request.current = controller;
    try {
      const response = await fetch(`/api/documents/${documentId}/index-job`, {
        method: "POST", headers: { "X-Requested-With": "personal-ai" },
        signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]),
      });
      const data = await response.json();
      if (controller.signal.aborted) return;
      if (!response.ok) {
        if (data?.error?.code === "indexing_disabled") { setState("disabled"); setJob(null); return; }
        throw new Error(response.status === 401 ? "登录已失效，请重新登录。" : "提交结果未确认，请先刷新状态后重试。");
      }
      setJob(data); setState("ready"); setRefresh(value => value + 1);
    } catch (e) {
      if (!controller.signal.aborted) { setState("error"); setError(e instanceof Error && e.name === "Error" ? e.message : "提交结果未确认，请先刷新状态后重试。"); }
    } finally { if (!controller.signal.aborted) setSubmitting(false); }
  }

  return <section className="indexJobPanel" aria-label={`${title}的索引`}>
    <p role="status">{state === "loading" ? "正在读取索引状态…" : state === "disabled" ? "管理员尚未启用索引。" : state === "error" ? "索引状态未确认。" : job ? `${labels[job.status]} · ${job.indexed_chunks}/${job.total_chunks} 块` : "当前目标暂无索引任务。"}</p>
    {job && state !== "error" && job.error_code && <p>{failures[job.error_code] ?? "索引处理失败"}；当前批次已尝试 {job.attempts} 次。</p>}
    {error && <p role="alert">{error}</p>}
    <button disabled={submitting || state !== "ready" || Boolean(active) || job?.status === "completed"} onClick={() => void submit()}>
      {submitting ? "正在提交…" : job?.status === "failed" ? "重试索引" : "建立索引"}
    </button>
    <button disabled={submitting || state === "loading"} onClick={() => { setState("loading"); setRefresh(value => value + 1); }}>刷新索引状态</button>
  </section>;
}
