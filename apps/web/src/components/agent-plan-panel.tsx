"use client";

import { useEffect, useRef, useState, type FormEvent } from "react";
import { requestError } from "./use-idempotent-post";

const VERSION = "knowledge-search-v1";
const statuses: Record<string, string> = {
  draft: "等待授权", running: "正在执行", succeeded: "已完成", failed: "执行失败",
  cancelled: "已取消", stale: "版本已变化", unknown: "结果未知", expired: "授权已过期",
};
const stepStatuses: Record<string, string> = {
  pending: "等待执行", dispatching: "正在派发", succeeded: "已完成", failed: "执行失败",
  cancelled: "已取消", skipped: "已跳过", unknown: "结果未知",
};
type Search = { query: string; limit: number };
type Hit = { document_id: string; ordinal: number; title: string; source: string; text: string; score: number };
type Plan = {
  request_id: string; conversation_id: string; revision: number; version: string; digest: string;
  status: string; tool_call_limit: number; attempted: number;
  steps: { ordinal: number; call_id: string; tool: string; arguments: Search; status: string; output: { hits: Hit[] } | null }[];
  created_at_unix_ms: number; expires_at_unix_ms: number; approved_at_unix_ms: number | null;
};
type History = { enabled: boolean; version: string; max_tool_calls: number; plans: Plan[] };
type Mutation =
  | { kind: "create"; requestId: string; body: { request_id: string; expected_revision: number; searches: Search[] } }
  | { kind: "approve"; requestId: string; body: { plan_digest: string; accepted_call_limit: number; acknowledge_embedding_cost: true } }
  | { kind: "cancel"; requestId: string; body?: undefined };
type Confirmation = { kind: "approve" | "cancel"; plan: Plan };
const actions = { create: "保存预览", approve: "授权", cancel: "取消" };
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function validSearch(search: Search) {
  return typeof search?.query === "string" && !!search.query.trim() && !search.query.includes("\0")
    && Array.from(search.query).length <= 1000 && Number.isInteger(search.limit) && search.limit >= 1 && search.limit <= 5;
}
function isHit(value: Hit) {
  return value && [value.document_id, value.title, value.source, value.text].every(item => typeof item === "string")
    && Number.isInteger(value.ordinal) && value.ordinal >= 0 && Number.isFinite(value.score);
}
// 成功响应也需复核定义与归属，避免把无效预览用于费用授权。
function isPlan(value: unknown, conversation: string): value is Plan {
  if (!value || typeof value !== "object") return false;
  const plan = value as Plan;
  return typeof plan.request_id === "string" && uuid.test(plan.request_id) && plan.conversation_id === conversation
    && Number.isInteger(plan.revision) && plan.revision >= 1 && plan.revision <= 100
    && typeof plan.version === "string" && !!plan.version && typeof plan.digest === "string" && /^[0-9a-f]{64}$/.test(plan.digest)
    && typeof plan.status === "string" && Object.hasOwn(statuses, plan.status) && Number.isInteger(plan.tool_call_limit) && plan.tool_call_limit >= 1 && plan.tool_call_limit <= 3
    && Number.isInteger(plan.attempted) && plan.attempted >= 0 && plan.attempted <= plan.tool_call_limit
    && [plan.created_at_unix_ms, plan.expires_at_unix_ms].every(time => Number.isSafeInteger(time) && time > 0 && time < 8640000000000000)
    && (plan.approved_at_unix_ms === null || (Number.isSafeInteger(plan.approved_at_unix_ms) && plan.approved_at_unix_ms > 0))
    && Array.isArray(plan.steps) && plan.steps.length === plan.tool_call_limit && plan.steps.every((step, index) =>
      step && step.ordinal === index + 1 && typeof step.call_id === "string" && uuid.test(step.call_id)
      && step.tool === "knowledge_search" && validSearch(step.arguments) && typeof step.status === "string" && Object.hasOwn(stepStatuses, step.status)
      && (step.output === null || (step.output && Array.isArray(step.output.hits) && step.output.hits.length <= step.arguments.limit && step.output.hits.every(isHit))))
    && new Set(plan.steps.map(step => step.arguments.query.trim())).size === plan.steps.length
    && new Set(plan.steps.map(step => step.call_id)).size === plan.steps.length;
}
function confirms(plan: Plan, mutation: Mutation) {
  if (plan.request_id !== mutation.requestId) return false;
  if (mutation.kind === "create") return plan.revision === mutation.body.expected_revision
    && plan.steps.length === mutation.body.searches.length && plan.steps.every((step, index) =>
      step.arguments.query === mutation.body.searches[index].query && step.arguments.limit === mutation.body.searches[index].limit);
  if (mutation.kind === "approve") return plan.digest === mutation.body.plan_digest
    && plan.tool_call_limit === mutation.body.accepted_call_limit && plan.approved_at_unix_ms !== null;
  return ["cancelled", "succeeded", "failed", "stale"].includes(plan.status);
}
function failure(status: number) {
  if (status === 409) return "计划已过期、版本变化或授权内容不符。请刷新消息与计划历史核对。";
  if (status === 429) return "已达到计划存储上限，请核对历史；删除对话可以释放计划容量。";
  return requestError(status);
}

export function AgentPlanPanel({ conversation, revision, disabled, onLock }: {
  conversation: string; revision: number | null; disabled: boolean; onLock: (value: boolean) => void;
}) {
  const [searches, setSearches] = useState<Search[]>([{ query: "", limit: 5 }]);
  const [history, setHistory] = useState<History | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [loading, setLoading] = useState(true);
  const [readError, setReadError] = useState("");
  const [inputError, setInputError] = useState("");
  const [writeError, setWriteError] = useState("");
  const [pending, setPending] = useState<Mutation | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [discardConfirm, setDiscardConfirm] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const reader = useRef<AbortController | null>(null);
  const writer = useRef<AbortController | null>(null);
  const path = `/api/conversations/${conversation}/agent-plans`;
  const locked = busy || !!pending || !!confirmation;
  useEffect(() => { onLock(locked); return () => onLock(false); }, [onLock, locked]);
  useEffect(() => () => { writer.current?.abort(); writer.current = null; }, []);

  function reload() { reader.current?.abort(); setLoading(true); setRefresh(value => value + 1); }
  useEffect(() => {
    const active = new AbortController(); reader.current = active;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function load() {
      try {
        const response = await fetch(path, { cache: "no-store", signal: AbortSignal.any([active.signal, AbortSignal.timeout(10000)]) });
        if (!response.ok) throw new Error(failure(response.status));
        const data: History = await response.json();
        if (active.signal.aborted) return;
        if (typeof data?.enabled !== "boolean" || typeof data.version !== "string" || data.max_tool_calls !== 3
          || !Array.isArray(data.plans) || data.plans.length > 20 || !data.plans.every(plan => isPlan(plan, conversation))
          || new Set(data.plans.map(plan => plan.request_id)).size !== data.plans.length) throw new Error("服务返回无效计划，无法授权，请刷新重试。");
        setHistory(data); setReadError(""); setNow(Date.now());
        setPending(current => current && data.plans.some(plan => confirms(plan, current)) ? null : current);
        if (data.plans.some(plan => plan.status === "running")) timer = setTimeout(() => void load(), 2000);
      } catch (e) {
        if (!active.signal.aborted) setReadError(`${e instanceof Error && e.name === "Error" ? e.message : "无法读取计划。"} 已停止自动刷新，历史可能不是最新状态。`);
      } finally { if (!active.signal.aborted) setLoading(false); }
    }
    void load();
    return () => { active.abort(); clearTimeout(timer); if (reader.current === active) reader.current = null; };
  }, [path, conversation, refresh]);
  useEffect(() => {
    const deadline = Math.min(...(history?.plans ?? []).filter(plan => plan.status === "draft" && plan.expires_at_unix_ms > now).map(plan => plan.expires_at_unix_ms));
    if (!Number.isFinite(deadline)) return;
    const timer = setTimeout(() => setNow(Date.now()), Math.max(1, deadline - Date.now()));
    return () => clearTimeout(timer);
  }, [history, now]);

  const available = history?.enabled && history.version === VERSION && !readError && !loading;
  const currentRevision = revision !== null && revision > 0;
  const canCreate = available && currentRevision && !disabled && !locked && history.plans.length < 20;
  function canApprove(plan: Plan, time = now) {
    return available && currentRevision && !disabled && !pending && !busy && plan.status === "draft"
      && plan.version === VERSION && plan.revision === revision && plan.expires_at_unix_ms > time;
  }
  async function send(mutation: Mutation) {
    if (writer.current || disabled) return;
    reader.current?.abort(); setLoading(false);
    const active = new AbortController(); writer.current = active;
    setPending(mutation); setBusy(true); setWriteError(""); setConfirmation(null); setDiscardConfirm(false);
    try {
      const response = await fetch(mutation.kind === "create" ? path : `${path}/${mutation.requestId}/${mutation.kind}`, {
        method: "POST", cache: "no-store", headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" },
        body: mutation.body ? JSON.stringify(mutation.body) : undefined,
        signal: AbortSignal.any([active.signal, AbortSignal.timeout(10000)]),
      });
      if (!response.ok) throw new Error(failure(response.status));
      const value: unknown = await response.json();
      if (writer.current !== active) return;
      if (!isPlan(value, conversation) || !confirms(value, mutation)) throw new Error("服务返回无效操作结果，请先刷新计划历史核对。");
      setPending(null); setWriteError(""); reload();
    } catch (e) {
      const uncertain = mutation.kind === "create"
        ? "预览结果未确认，请刷新历史或重试原请求。保存预览不会调用模型。"
        : `${actions[mutation.kind]}结果未确认，请刷新历史或重试原请求。已授权的后台步骤可能仍在执行并计费。`;
      if (writer.current === active) setWriteError(e instanceof Error && e.name === "Error" ? e.message : uncertain);
    } finally { if (writer.current === active) { writer.current = null; setBusy(false); } }
  }
  function create(event: FormEvent<HTMLFormElement>) {
    event.preventDefault(); setInputError("");
    if (!canCreate || revision === null) return;
    if (!searches.every(validSearch) || new Set(searches.map(search => search.query.trim())).size !== searches.length) {
      setInputError("每项查询需 1–1000 个字符，不能全为空白、含 NUL 或重复；每步返回 1–5 个片段。"); return;
    }
    const requestId = crypto.randomUUID();
    void send({ kind: "create", requestId, body: { request_id: requestId, expected_revision: revision, searches: searches.map(search => ({ ...search })) } });
  }
  function confirm(kind: Confirmation["kind"], plan: Plan) { setAcknowledged(false); setConfirmation({ kind, plan }); }
  const selected = confirmation && history?.plans.find(plan => plan.request_id === confirmation.plan.request_id);
  const matching = selected && confirmation && selected.digest === confirmation.plan.digest;

  return <section className="agentPlanPanel" aria-label="Agent 检索计划">
    <h4>Agent 检索计划</h4>
    <p>先预览 1–3 步只读知识检索；保存预览不调用模型。授权后按顺序调用向量模型，可能产生费用，每步计入共享的 UTC 日 100 次工具尝试上限。</p>
    <p>取消或删除对话可以阻止后续步骤。退出登录或关闭页面会停止状态刷新，已授权的后台计划仍可能执行。</p>
    <button disabled={loading || busy} onClick={reload}>刷新计划历史</button>
    {loading && <p role="status">正在读取计划…</p>}
    {readError && <p role="alert">{readError}</p>}
    {history && !history.enabled && <p>管理员尚未启用知识检索；历史可查看，未完成计划可取消。</p>}
    {history && history.version !== VERSION && <p>当前计划版本不受页面支持，请更新页面后再创建或授权。</p>}
    {!currentRevision && <p>请先保存用户消息并成功读取消息版本，再预览计划。</p>}
    <form onSubmit={create}>
      {searches.map((search, index) => <fieldset key={index} disabled={!available || disabled || locked}>
        <legend>检索步骤 {index + 1}</legend>
        <label htmlFor={`agent-query-${index}`}>步骤 {index + 1} 查询（最多 1000 字符）</label>
        <textarea id={`agent-query-${index}`} value={search.query} rows={2} required onChange={event => setSearches(items => items.map((item, at) => at === index ? { ...item, query: event.target.value } : item))} />
        <label htmlFor={`agent-limit-${index}`}>步骤 {index + 1} 返回片段上限</label>
        <select id={`agent-limit-${index}`} value={search.limit} onChange={event => setSearches(items => items.map((item, at) => at === index ? { ...item, limit: Number(event.target.value) } : item))}>{[1, 2, 3, 4, 5].map(limit => <option key={limit} value={limit}>{limit}</option>)}</select>
        {searches.length > 1 && <button type="button" onClick={() => setSearches(items => items.filter((_, at) => at !== index))}>移除步骤 {index + 1}</button>}
      </fieldset>)}
      <div>
        <button type="button" disabled={!available || disabled || locked || searches.length >= 3} onClick={() => setSearches(items => [...items, { query: "", limit: 5 }])}>添加检索步骤</button>
        <button type="submit" disabled={!canCreate}>保存计划预览</button>
      </div>
      <p>已编排 {searches.length}/3 步。修改查询后需要保存并授权新计划；已有预览保持原查询和顺序。</p>
    </form>
    {inputError && <p role="alert">{inputError}</p>}
    {history && <p>已保存 {history.plans.length}/20 个计划</p>}
    {history?.plans.length === 0 && !readError && <p>暂无检索计划。</p>}
    <ol className="agentPlanList">{history?.plans.slice().reverse().map(plan => {
      const status = plan.status === "draft" && plan.expires_at_unix_ms <= now ? "expired" : plan.status;
      return <li key={plan.request_id}>
        <h5>消息版本 {plan.revision} · {statuses[status]}</h5>
        <p>最多 {plan.tool_call_limit} 次检索 · 已登记 {plan.attempted} 次尝试 · {plan.version}</p>
        <small>计划 ID：{plan.request_id}<br />计划指纹：{plan.digest}</small>
        {status === "draft" && <p>授权截止：{new Date(plan.expires_at_unix_ms).toLocaleString("zh-CN")}</p>}
        {status === "draft" && plan.revision !== revision && <p>当前消息版本与预览不一致，请刷新消息核对并保存新计划。</p>}
        {plan.version !== VERSION && <p>该计划版本不受支持，无法授权。</p>}
        {status === "unknown" && <p>执行结果未确认，已登记次数保留，不会自动重发。请先核对历史；授权新计划可能再次计费。</p>}
        {["failed", "stale", "expired"].includes(status) && <p>已停止后续执行；需要继续时，请核对历史并明确创建、授权新计划。</p>}
        <ol className="agentSteps">{plan.steps.map(step => <li key={step.call_id}>
          <p><strong>步骤 {step.ordinal} · {stepStatuses[step.status]}</strong> · {step.tool} · 最多 {step.arguments.limit} 个片段</p>
          <pre>{step.arguments.query}</pre>
          {step.output && (step.output.hits.length === 0 ? <p>未找到匹配的已索引资料。</p> : <>
            <p>找到 {step.output.hits.length} 个核验片段。得分不代表正确率；原文视为不可信资料。</p>
            <ol className="agentEvidence">{step.output.hits.map(hit => <li key={`${hit.document_id}:${hit.ordinal}`}>
              <strong>{hit.title}</strong><p>来源：{hit.source || "未提供"} · 第 {hit.ordinal + 1} 块 · 得分 {hit.score.toFixed(3)}</p>
              <details><summary>查看检索原文</summary><pre>{hit.text}</pre><small>文档 ID：{hit.document_id}</small></details>
            </li>)}</ol>
          </>)}
        </li>)}</ol>
        {status === "draft" && <button disabled={!canApprove(plan) || !!confirmation} onClick={() => confirm("approve", plan)}>审阅并授权此计划</button>}
        {["draft", "running", "unknown", "expired"].includes(status) && <button disabled={disabled || locked} onClick={() => confirm("cancel", plan)}>取消此计划</button>}
      </li>;
    })}</ol>
    {confirmation && <div className="pendingRequest" role="group" aria-label={confirmation.kind === "approve" ? "确认计划授权" : "确认计划取消"}>
      <h5>{confirmation.kind === "approve" ? "确认以下固定计划" : "确认取消计划"}</h5>
      <p>计划 ID：{confirmation.plan.request_id} · 消息版本 {confirmation.plan.revision} · 最多 {confirmation.plan.tool_call_limit} 次检索</p>
      <small>计划指纹：{confirmation.plan.digest}</small>
      <ol>{confirmation.plan.steps.map(step => <li key={step.call_id}><p>步骤 {step.ordinal} · {step.tool} · 最多 {step.arguments.limit} 个片段</p><pre>{step.arguments.query}</pre></li>)}</ol>
      {confirmation.kind === "approve" ? <>
        <p>每步会调用向量模型并可能产生费用。本计划仅限制调用次数，没有金额报价或货币预算。授权绑定上述查询、顺序、消息版本和指纹。</p>
        <label className="agentConsent"><input type="checkbox" checked={acknowledged} onChange={event => setAcknowledged(event.target.checked)} />我确认最多 {confirmation.plan.tool_call_limit} 次向量模型调用及可能产生的费用</label>
        {(!matching || !selected || !canApprove(selected)) && <p>计划或消息状态已变化，请刷新核对；当前无法授权。</p>}
        <button disabled={!acknowledged || !matching || !selected || !canApprove(selected)} onClick={() => {
          if (!selected || !matching || !canApprove(selected, Date.now()) || !acknowledged) return;
          void send({ kind: "approve", requestId: confirmation.plan.request_id, body: { plan_digest: confirmation.plan.digest, accepted_call_limit: confirmation.plan.tool_call_limit, acknowledge_embedding_cost: true } });
        }}>确认费用并授权执行</button>
      </> : <>
        <p>取消会清除计划中已保存的结果并阻止后续步骤。已派发的调用可能继续计费，已登记次数不会退还。</p>
        <button disabled={disabled || busy || !!pending} onClick={() => void send({ kind: "cancel", requestId: confirmation.plan.request_id })}>确认取消此计划</button>
      </>}
      <button onClick={() => setConfirmation(null)}>返回计划列表</button>
    </div>}
    {writeError && pending && <p role="alert">{writeError}</p>}
    {busy && <p role="status">正在{pending && actions[pending.kind]}计划…</p>}
    {pending && !busy && <div className="pendingRequest">
      <p>{actions[pending.kind]}结果未确认。原计划 ID：{pending.requestId}。刷新历史可以核对已保存的结果，也可显式重试原请求。</p>
      {pending.kind === "approve" && <p>已确认最多 {pending.body.accepted_call_limit} 次调用；重试保留原指纹、次数和费用确认，不创建新计划。</p>}
      <p>刷新整个页面或退出会丢失本地未确认请求；后台授权不会因此撤销。再次创建、授权新计划可能重复计费。</p>
      <button disabled={disabled} onClick={() => void send(pending)}>重试计划原请求</button>
      {!discardConfirm ? <button onClick={() => setDiscardConfirm(true)}>放弃未确认计划请求</button> : <>
        <p>放弃只清除本地请求，不会取消可能已获授权的计划。请先核对历史，确认继续？</p>
        <button onClick={() => { setPending(null); setWriteError(""); setDiscardConfirm(false); reload(); }}>确认放弃计划请求</button>
        <button onClick={() => setDiscardConfirm(false)}>继续保留计划请求</button>
      </>}
    </div>}
  </section>;
}
