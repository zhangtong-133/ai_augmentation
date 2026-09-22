"use client";

import { useEffect, useRef, useState } from "react";
import { requestError, useIdempotentPost } from "./use-idempotent-post";

type Reply = { request_id: string; revision: number; status: "queued" | "dispatching" | "succeeded" | "failed" | "unknown" | "cancelled"; output: string | null };
type History = { enabled: boolean; mode: "fixture"; items: Reply[] };
const labels: Record<Reply["status"], string> = { queued: "等待执行", dispatching: "正在执行", succeeded: "已完成", failed: "执行失败", unknown: "结果未知", cancelled: "已取消" };
const active = (item: Reply) => item.status === "queued" || item.status === "dispatching";

export function ReplyPanel({ conversation, revision, disabled, onLock }: { conversation: string; revision: number | null; disabled: boolean; onLock: (value: boolean) => void }) {
  const path = `/api/conversations/${conversation}/replies`;
  const [history, setHistory] = useState<History | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [loading, setLoading] = useState(true);
  const [readError, setReadError] = useState("");
  const [cancelError, setCancelError] = useState("");
  const [cancelBusy, setCancelBusy] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [discardConfirm, setDiscardConfirm] = useState(false);
  const readController = useRef<AbortController | null>(null);
  const cancelController = useRef<AbortController | null>(null);
  function reload() { readController.current?.abort(); setLoading(true); setReadError(""); setRefresh(value => value + 1); }
  const post = useIdempotentPost<Reply>(path, () => { setConfirm(false); reload(); });
  const locked = post.busy || !!post.pending || cancelBusy;
  useEffect(() => { onLock(locked); return () => onLock(false); }, [locked, onLock]);
  useEffect(() => () => { cancelController.current?.abort(); cancelController.current = null; }, []);
  useEffect(() => {
    const controller = new AbortController(); readController.current = controller;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function read() {
      try {
        const response = await fetch(path, { cache: "no-store", signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) });
        if (!response.ok) throw new Error(requestError(response.status));
        const data: History = await response.json();
        if (controller.signal.aborted) return;
        setHistory(data); setReadError(""); setLoading(false);
        if (data.items.some(active)) timer = setTimeout(() => void read(), 2000);
      } catch (error) {
        if (!controller.signal.aborted) {
          setReadError(error instanceof Error && error.name === "Error" ? error.message : "无法读取回复历史，请手动刷新。");
          setLoading(false);
        }
      }
    }
    void read();
    return () => { controller.abort(); clearTimeout(timer); };
  }, [path, refresh]);

  async function cancel(request: string) {
    if (cancelController.current) return;
    const controller = new AbortController(); cancelController.current = controller;
    setCancelBusy(true); setCancelError("");
    try {
      const response = await fetch(`${path}/${request}/cancel`, { method: "POST", headers: { "X-Requested-With": "personal-ai" }, signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) });
      if (!response.ok) throw new Error(requestError(response.status));
      if (cancelController.current === controller) reload();
    } catch (error) {
      if (cancelController.current === controller) setCancelError(error instanceof Error && error.name === "Error" ? error.message : "取消结果未确认，请刷新核对或再次取消。");
    } finally { if (cancelController.current === controller) { cancelController.current = null; setCancelBusy(false); } }
  }
  const canCreate = !disabled && !locked && !loading && !readError && history?.enabled && revision !== null && revision > 0 && history.items.length < 100 && !history.items.some(active);
  return <section className="replyPanel" aria-label="显式回复">
    <h4>显式回复 · 本地测试</h4>
    <p>不是模型生成，不会产生模型费用。仅回显最后一条已保存的用户消息，不读取知识库或长期记忆。</p>
    <p>每账户每日最多预留 20 次，每个对话累计最多 100 个请求。取消已派发请求不退回次数，切换页面不会取消后台任务。</p>
    <button disabled={loading || post.busy || cancelBusy} onClick={reload}>刷新回复历史</button>
    {loading && <p role="status">正在读取回复…</p>}
    {readError && <p role="alert">{readError} 已停止自动刷新；下方可能为旧状态。</p>}
    {history && !history.enabled && <p>管理员尚未启用测试回复，仍可查询或取消已有请求。</p>}
    {!readError && !loading && history?.items.length === 0 && <p>暂无回复请求。</p>}
    <ol className="replyList">{history?.items.map(item => <li key={item.request_id}>
      <p>消息版本 {item.revision} · {labels[item.status]}</p>
      <small>请求 ID：{item.request_id}</small>
      {item.output !== null && <p className="replyOutput">{item.output}</p>}
      {item.status === "unknown" && <p>结果未知，不会自动重发。再次请求会使用新 ID 并占用一次额度。</p>}
      {active(item) && <button disabled={disabled || post.busy || cancelBusy} onClick={() => void cancel(item.request_id)}>取消此回复</button>}
    </li>)}</ol>
    {cancelError && <p role="alert">{cancelError}</p>}
    {cancelBusy && <p role="status">正在取消回复…</p>}
    <button disabled={!canCreate} onClick={() => setConfirm(true)}>请求测试回复</button>
    {confirm && !post.pending && <div className="pendingRequest">
      <p>确认针对已保存消息版本 {revision} 创建一次新测试请求？已有回复不会重新加入上下文。</p>
      <button disabled={!canCreate} onClick={() => { setConfirm(false); void post.submit({ expected_revision: revision! }); }}>确认请求测试回复</button>
      <button disabled={post.busy} onClick={() => setConfirm(false)}>暂不请求</button>
    </div>}
    {post.error && <p role="alert">{post.error} 请先刷新消息和回复历史核对版本、额度及服务状态。</p>}
    {post.busy && <p role="status">正在提交回复请求…</p>}
    {post.pending && !post.busy && <div className="pendingRequest">
      <p>原请求 ID：{post.pending.request_id}；消息版本 {post.pending.expected_revision}。只会显式重试相同请求。</p>
      <p>刷新整个页面或退出会丢失本地请求 ID；再次请求前请核对回复历史。丢失响应不代表后台未执行。</p>
      <button disabled={disabled || cancelBusy} onClick={() => void post.submit({})}>重试回复原请求</button>
      {!discardConfirm ? <button onClick={() => setDiscardConfirm(true)}>放弃未确认回复</button> : <>
        <p>放弃不等于取消，可能已执行。确认仅清除本地未确认请求？</p>
        <button onClick={() => { post.discard(); setDiscardConfirm(false); reload(); }}>确认放弃回复请求</button>
        <button onClick={() => setDiscardConfirm(false)}>继续保留回复请求</button>
      </>}
    </div>}
  </section>;
}
