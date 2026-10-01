"use client";

import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";

type Schedule = { revision: string; enabled: boolean; minute_utc: number; next_run_unix_ms: string | null; last_attempt_unix_ms: string | null; last_request_id: string | null; last_outcome: "generated" | "skipped" | null };
const path = "/api/feed-brief-schedule";
const clock = (minute: number) => `${String(Math.floor(minute / 60)).padStart(2, "0")}:${String(minute % 60).padStart(2, "0")}`;
function date(value: string | null) { if (value === null) return "无"; const d = new Date(Number(value)); return Number.isFinite(d.getTime()) ? d.toISOString().replace("T", " ").replace(".000Z", " UTC") : "时间不可显示"; }

export function BriefSchedule({ onExpired }: { onExpired: () => void }) {
  const [saved, setSaved] = useState<Schedule | null>(null);
  const [enabled, setEnabled] = useState(false);
  const [time, setTime] = useState("09:00");
  const [busy, setBusy] = useState(false);
  const [uncertain, setUncertain] = useState(false);
  const [error, setError] = useState("");
  const active = useRef<AbortController | null>(null);
  const alive = useRef(true);
  const valid = useCallback((c: AbortController) => alive.current && active.current === c && !c.signal.aborted, []);
  const run = useCallback(async (body?: object) => {
    if (active.current) return;
    const c = new AbortController(); active.current = c; setBusy(true); setError("");
    try {
      const response = await fetch(path, { method: body ? "PUT" : "GET", cache: "no-store", headers: body ? { "Content-Type": "application/json", "X-Requested-With": "personal-ai" } : undefined, body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.any([c.signal, AbortSignal.timeout(10000)]) });
      if (!valid(c)) return;
      if (response.status === 401) { setSaved(null); onExpired(); return; }
      if (!response.ok) throw new Error(response.status === 409 ? "定时设置已变化，请刷新后核对。" : "无法确认定时设置，请刷新核对保存结果。");
      const data: Schedule = await response.json();
      if (!valid(c)) return;
      setSaved(data); setEnabled(data.enabled); setTime(clock(data.minute_utc)); setUncertain(false);
    } catch (e) {
      if (valid(c)) { if (body) setUncertain(true); setError(e instanceof Error && e.name === "Error" ? e.message : "连接中断，请刷新核对定时设置。"); }
    } finally { if (valid(c)) { active.current = null; setBusy(false); } }
  }, [valid, onExpired]);
  useEffect(() => {
    alive.current = true;
    const timer = window.setTimeout(() => void run(), 0);
    return () => { alive.current = false; window.clearTimeout(timer); active.current?.abort(); active.current = null; };
  }, [run]);
  function save(event: FormEvent) {
    event.preventDefault();
    if (!saved || busy || uncertain || !/^\d{2}:\d{2}$/.test(time)) return;
    const [hour, minute] = time.split(":").map(Number);
    if (hour > 23 || minute > 59) return;
    void run({ revision: saved.revision, enabled, minute_utc: hour * 60 + minute, acknowledge_schedule: enabled });
  }
  const locked = busy || !saved || uncertain;
  return <section aria-label="定时日报" className="knowledgePanel">
    <h3>定时日报</h3>
    <p>启用后，每个 UTC 日最多生成一次，使用运行时的关键词偏好和当天已保存条目。不会自动采集、调用模型或发送通知；需要管理员运行调度服务。</p>
    {saved && <><p>当前状态：{saved.enabled ? "已启用" : "已停用"}；计划时间：{clock(saved.minute_utc)} UTC；下次计划：{date(saved.next_run_unix_ms)}</p>
      <p>最近处理：{date(saved.last_attempt_unix_ms)}；结果：{saved.last_outcome === "generated" ? "已生成，可刷新日报历史查看" : saved.last_outcome === "skipped" ? "因额度或条目限制跳过，可手动生成或等待下一天" : "尚未处理"}</p></>}
    <form onSubmit={save}>
      <label><input type="checkbox" checked={enabled} disabled={locked} onChange={e => setEnabled(e.target.checked)} />每天自动生成日报</label>
      <label>每日生成时间（UTC）<input type="time" required value={time} disabled={locked} onChange={e => setTime(e.target.value)} /></label>
      <p>首次启用或修改时间从下一个计划时刻开始，不补生成过去日期；停用不会删除已有日报。</p>
      <button disabled={locked}>{enabled ? "确认启用定时日报" : "保存停用设置"}</button>
    </form>
    {uncertain && <p role="status">保存结果尚未确认，请先刷新核对；不会自动重新提交。</p>}
    <button disabled={busy} onClick={() => void run()}>刷新定时设置</button>
    {error && <p role="alert">{error}</p>}
  </section>;
}
