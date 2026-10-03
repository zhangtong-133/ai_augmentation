"use client";
import { useEffect, useRef, useState } from "react";
import { observeLearningStatus, StatusHttpError, type LearningStatus } from "../lib/learning-status-stream";
export function LearningStatusObserver({ request, enabled, begin, observed, refresh, expire }: {
  request: string; enabled: boolean; begin: () => void; observed: (status: LearningStatus) => void; refresh: () => void; expire: () => void;
}) {
  const active = useRef<AbortController | null>(null);
  const [watching, setWatching] = useState(false); const [notice, setNotice] = useState("");
  useEffect(() => () => { active.current?.abort(); active.current = null; }, [request]);
  async function start() {
    if (active.current || !enabled) return;
    const c = new AbortController(); active.current = c; setWatching(true); setNotice("正在观察执行状态，最长 20 秒。"); begin();
    const valid = () => active.current === c && !c.signal.aborted;
    let query = false;
    try {
      await observeLearningStatus(request, c.signal, event => {
        if (!valid()) return;
        if (event.type === "status") { observed(event.status); query = event.terminal; }
        else if (event.reason === "session_unavailable") { c.abort(); expire(); }
        else if (event.reason === "observation_timeout") query = true;
        else { setNotice("状态暂不可用，请手动核对原授权。"); }
      });
      if (valid()) {
        setWatching(false); active.current = null;
        if (query) { setNotice("观察已结束，正在核对保存状态。"); refresh(); }
      }
    } catch (error) {
      if (!valid()) return;
      if (error instanceof StatusHttpError && error.status === 401) expire();
      else setNotice(error instanceof StatusHttpError && error.status === 429 ? "观察连接已达上限，请稍后再试或手动核对状态。" : "观察已中断，请手动核对原授权。不会自动重连或重发。");
    } finally {
      if (active.current === c) { active.current = null; setWatching(false); }
    }
  }
  function stop() { active.current?.abort(); active.current = null; setWatching(false); setNotice("已停止观察。执行状态仍可手动核对，停止观察不会取消模型授权。"); }
  return <section aria-label="模型执行状态观察">
    <p>观察只读取本次状态，不会执行模型或消耗模型额度。结束后核对已保存的建议。</p>
    {watching ? <button onClick={stop}>停止观察执行状态</button> : <button disabled={!enabled} onClick={() => void start()}>观察执行状态</button>}
    {notice && <p role="status">{notice}</p>}
  </section>;
}
