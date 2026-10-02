"use client";

import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";

import { ReminderInbox } from "./reminder-inbox";

type Schedule = { request_id: string; title: string; body: string; run_at_unix_ms: string; digest: string; status: string; max_runs: number; amount_micro: string; approval_expires_at_unix_ms: string };

type Page<T> = { items: T[]; next_cursor: string | null };
type Mutation = { path: string; body: string; kind: "create" | "approve" | "cancel" };
const status: Record<string, string> = { draft: "待确认", expired: "预览已过期", scheduled: "等待投递", running: "正在投递", delivered: "已投递", cancelled: "已取消", failed: "投递停止" };
function time(value: string) { return new Date(Number(value)).toLocaleString(undefined, { timeZoneName: "short" }); }
function errorMessage(code: number) {
  if (code === 401) return "登录已失效，请重新登录。";
  if (code === 404) return "提醒任务不存在或无权访问。";
  if (code === 409) return "任务状态已变化、预览已过期或创建额度已满，请刷新后重新审阅。";
  if (code === 400 || code === 422) return "请检查标题、正文和提醒时间；时间须在 1 分钟至 365 天之后。";
  return "服务暂不可用，操作结果尚未确认。请重试原操作。";
}
function localTime(value: string) {
  const date = new Date(value);
  const pad = (n: number) => String(n).padStart(2, "0");
  const roundtrip = `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
  if (!Number.isFinite(date.getTime()) || roundtrip !== value) throw new Error("该本地时间无效，请重新选择（夏令时切换可能跳过部分时间）。");
  return date.getTime();
}

export function SchedulePanel() {
  const [tasks, setTasks] = useState<Page<Schedule>>({ items: [], next_cursor: null });
  const [taskPages, setTaskPages] = useState<string[]>([]);
  const [revision, setRevision] = useState(0);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<Mutation | null>(null);
  const [selected, setSelected] = useState<Schedule | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [cancelId, setCancelId] = useState<string | null>(null);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [when, setWhen] = useState("");
  const [error, setError] = useState("");
  const [loadError, setLoadError] = useState("");
  const [notice, setNotice] = useState("");
  const active = useRef<AbortController | null>(null);
  const taskCursor = taskPages.at(-1);
  const [expired, setExpired] = useState(false);
  const sessionExpired = useRef(false);
  const expire = useCallback(() => { sessionExpired.current = true; setExpired(true); setTasks({ items: [], next_cursor: null }); setSelected(null); setPending(null); setTitle(""); setBody(""); setWhen(""); setNotice(""); setAcknowledged(false); setCancelId(null); active.current?.abort(); active.current = null; setBusy(false); setLoading(false); setLoadError("登录已失效，请重新登录。"); }, []);
  useEffect(() => () => { active.current?.abort(); active.current = null; }, []);
  useEffect(() => {
    const controller = new AbortController();
    async function load() {
      setLoading(true);
      try {
        const response = await fetch(`/api/schedules${taskCursor ? `?after=${encodeURIComponent(taskCursor)}` : ""}`, { cache: "no-store", signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) });
        if (controller.signal.aborted || sessionExpired.current) return;
        if (response.status === 401) { expire(); return; }
        if (!response.ok) throw new Error(response.status >= 500 ? "提醒服务暂不可用，请刷新重试。" : errorMessage(response.status));
        const schedules = await response.json();
        if (!controller.signal.aborted && !sessionExpired.current) { setTasks(schedules); setLoadError(""); }
      } catch (e) {
        if (!controller.signal.aborted && !sessionExpired.current) setLoadError(e instanceof Error && e.name === "Error" ? e.message : "无法读取提醒，请刷新重试。");
      } finally { if (!controller.signal.aborted) setLoading(false); }
    }
    if (!sessionExpired.current) void load();
    return () => controller.abort();
  }, [revision, taskCursor, expire]);

  async function mutate(operation: Mutation) {
    if (active.current) return;
    const controller = new AbortController(); active.current = controller;
    setPending(operation); setBusy(true); setError(""); setNotice("");
    try {
      const response = await fetch(operation.path, { method: "POST", headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" }, body: operation.body, signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) });
      if (active.current !== controller) return;
      if (response.status === 401) { expire(); return; }
      if (!response.ok) {
        if (response.status >= 400 && response.status < 500) { setPending(null); setSelected(null); setAcknowledged(false); setRevision(n => n + 1); }
        throw new Error(errorMessage(response.status));
      }
      const result: Schedule = await response.json();
      if (active.current !== controller) return;
      setPending(null); setSelected(result); setAcknowledged(false); setCancelId(null);
      setNotice(operation.kind === "create" ? (result.status === "draft" ? "预览已保存，尚未授权投递。" : `已有任务状态：${status[result.status] ?? result.status}。`) : operation.kind === "approve" ? `任务${status[result.status] ?? result.status}。` : result.status === "delivered" ? "提醒已投递，无法撤回。" : "任务已取消。");
      if (operation.kind === "create") { setTitle(""); setBody(""); setWhen(""); }
      setTaskPages([]); setRevision(n => n + 1);
    } catch (e) {
      if (active.current === controller) setError(e instanceof Error && e.name === "Error" ? e.message : "操作结果尚未确认，请重试原操作。不要重复创建提醒。");
    } finally { if (active.current === controller) { active.current = null; setBusy(false); } }
  }
  function create(event: FormEvent) {
    event.preventDefault();
    if (pending) return;
    try {
      if (!title.trim() || !body.trim() || [...title].length > 80 || [...body].length > 2000) throw new Error(errorMessage(400));
      const runAt = localTime(when);
      if (runAt < Date.now() + 60000 || runAt > Date.now() + 365 * 86400000) throw new Error(errorMessage(400));
      void mutate({ kind: "create", path: "/api/schedules", body: JSON.stringify({ request_id: crypto.randomUUID(), title, body, run_at_unix_ms: String(runAt) }) });
    } catch (e) { setError((e as Error).message); }
  }
  function approve() {
    if (!selected || !acknowledged || pending) return;
    void mutate({ kind: "approve", path: `/api/schedules/${selected.request_id}/approve`, body: JSON.stringify({ digest: selected.digest, accepted_run_at_unix_ms: selected.run_at_unix_ms, accepted_max_runs: selected.max_runs, accepted_amount_micro: selected.amount_micro, acknowledge_schedule: true }) });
  }
  const locked = expired || busy || pending !== null;
  return <section className="schedulePanel" aria-label="定时提醒">
    <h2>定时提醒</h2>
    <p>一次性站内提醒，免费，不调用模型或发送外部通知。确认后等待投递；服务离线时可能延迟。</p>
    <form onSubmit={create}>
      <label htmlFor="schedule-title">提醒标题</label><input id="schedule-title" required value={title} disabled={locked} onChange={e => setTitle(e.target.value)} />
      <label htmlFor="schedule-body">提醒内容</label><textarea id="schedule-body" required rows={3} value={body} disabled={locked} onChange={e => setBody(e.target.value)} />
      <label htmlFor="schedule-time">提醒时间（当前设备时区）</label><input id="schedule-time" type="datetime-local" required value={when} disabled={locked} onChange={e => setWhen(e.target.value)} />
      <button disabled={locked}>创建提醒预览</button>
    </form>
    {error && <p role="alert">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    {pending && <div className="scheduleReview"><p>原操作：{pending.kind === "create" ? "创建预览" : pending.kind === "approve" ? "确认投递" : "取消提醒"}。结果未确认时不能开始新操作。</p><button disabled={busy} onClick={() => void mutate(pending)}>{busy ? "正在提交…" : "重试原操作"}</button></div>}
    {selected && <div className="scheduleReview" aria-label="提醒预览">
      <h3>{selected.title}</h3><p className="memoryText">{selected.body}</p>
      <p>提醒时间：{time(selected.run_at_unix_ms)}</p><p>UTC：{new Date(Number(selected.run_at_unix_ms)).toISOString()}</p>
      <p>执行 1 次 · 费用 0 · {status[selected.status] ?? selected.status}</p>
      {selected.status === "draft" && <><p>确认期限：{time(selected.approval_expires_at_unix_ms)}</p><label><input type="checkbox" checked={acknowledged} disabled={locked} onChange={e => setAcknowledged(e.target.checked)} />我已核对时间和内容，同意一次性站内提醒</label><button disabled={locked || !acknowledged} onClick={approve}>确认投递提醒</button></>}
      <button disabled={locked} onClick={() => { setSelected(null); setAcknowledged(false); }}>关闭预览</button>
    </div>}
    {loadError && <p role="alert">{loadError}</p>}
    <button disabled={expired || busy || loading} onClick={() => setRevision(n => n + 1)}>刷新提醒</button>
    {loading && <p role="status">正在读取提醒…</p>}
    <h3>提醒任务</h3>
    {!loading && !loadError && tasks.items.length === 0 && <p>暂无提醒任务。</p>}
    <ul>{tasks.items.map(task => <li key={task.request_id}><h4>{task.title}</h4><p>{time(task.run_at_unix_ms)} · {status[task.status] ?? task.status}</p>
      <button disabled={locked} onClick={() => { setSelected(task); setAcknowledged(false); }}>审阅提醒</button>
      {!["cancelled", "delivered"].includes(task.status) && <button disabled={locked} onClick={() => setCancelId(task.request_id)}>取消提醒</button>}
      {cancelId === task.request_id && <div><p>确认取消此提醒？已投递的提醒无法撤回。</p><button disabled={locked} onClick={() => void mutate({ kind: "cancel", path: `/api/schedules/${task.request_id}/cancel`, body: "{}" })}>确认取消提醒</button><button disabled={locked} onClick={() => setCancelId(null)}>保留提醒</button></div>}
    </li>)}</ul>
    <div className="pagination"><button disabled={locked || loading || taskPages.length === 0} onClick={() => setTaskPages(p => p.slice(0, -1))}>上一页任务</button><button disabled={locked || loading || !tasks.next_cursor} onClick={() => setTaskPages(p => [...p, tasks.next_cursor!])}>下一页任务</button></div>
    {!expired && <ReminderInbox revision={revision} onExpired={expire} />}
  </section>;
}
