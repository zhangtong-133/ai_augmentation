"use client";
import { useEffect, useRef, useState } from "react";
import { observeLearningStatus, StatusHttpError, type LearningStatus } from "../lib/learning-status-stream";
import { observeLearningText, type TextEvent } from "../lib/learning-text-stream";
export function LearningStatusObserver({ request, enabled, begin, observed, refresh, expire }: {
  request: string; enabled: boolean; begin: () => void; observed: (status: LearningStatus) => void; refresh: () => void; expire: () => void;
}) {
  const active = useRef<AbortController | null>(null);
  const [watching, setWatching] = useState(false); const [notice, setNotice] = useState("");
  const [textMode, setTextMode] = useState(false); const [temporary, setTemporary] = useState("");
  useEffect(() => () => { active.current?.abort(); active.current = null; }, [request]);
  async function start(withText = false) {
    if (active.current || !enabled) return;
    const c = new AbortController(); active.current = c; setWatching(true); setTextMode(withText); setTemporary("");
    setNotice(withText ? "正在观察临时文本，最长 20 秒。" : "正在观察执行状态，最长 20 秒。"); begin();
    const valid = () => active.current === c && !c.signal.aborted;
    let query = false;
    try {
      const observe = withText ? observeLearningText : observeLearningStatus;
      await observe(request, c.signal, (event: TextEvent) => {
        if (!valid()) return;
        if (event.type === "delta") setTemporary(previous => previous + event.text);
        else if (event.type === "clear") { setTemporary(""); setNotice("临时文本已清除，正在等待保存状态。"); }
        else if (event.type === "status") { observed(event.status); query = event.terminal; if (event.terminal) setTemporary(""); }
        else {
          setTemporary("");
          if (event.reason === "session_unavailable") { c.abort(); expire(); }
          else if (event.reason === "observation_timeout") query = true;
          else setNotice("状态暂不可用，请手动核对原授权。");
        }
      });
      if (valid()) {
        setTemporary(""); setWatching(false); active.current = null;
        if (query) { setNotice("观察已结束，正在核对保存状态。"); refresh(); }
      }
    } catch (error) {
      if (!valid()) return;
      setTemporary("");
      if (error instanceof StatusHttpError && error.status === 401) expire();
      else if (withText && error instanceof StatusHttpError && error.status === 503) setNotice("临时文本当前不可用，请使用状态观察或手动核对原授权。");
      else setNotice(error instanceof StatusHttpError && error.status === 429 ? "观察连接已达上限，请稍后再试或手动核对状态。" : "观察已中断，请手动核对原授权。不会自动重连或重发。");
    } finally {
      if (active.current === c) { active.current = null; setWatching(false); setTemporary(""); }
    }
  }
  function stop() { active.current?.abort(); active.current = null; setWatching(false); setTemporary(""); setNotice("已停止观察。执行状态仍可手动核对，停止观察不会取消模型授权。"); }
  return <section aria-label="模型执行状态观察">
    <p>观察只读取本次状态或临时文本，不会执行模型或消耗模型额度。结束后核对已保存的建议。</p>
    {watching ? <button onClick={stop}>{textMode ? "停止观察临时文本" : "停止观察执行状态"}</button> : <>
      <button disabled={!enabled} onClick={() => void start()}>观察执行状态</button>
      <button disabled={!enabled} onClick={() => void start(true)}>观察临时文本</button>
    </>}
    {watching && textMode && <section aria-label="模型临时文本">
      <p>未校验的临时文本，仅含本次观察期间收到的内容。最终核验建议以保存结果为准，不会自动修改自评。</p>
      {temporary && <pre className="feedText" style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere", maxHeight: "16rem", overflowY: "auto" }}>{temporary}</pre>}
    </section>}
    {notice && <p role="status">{notice}</p>}
  </section>;
}
