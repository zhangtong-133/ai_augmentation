"use client";

import { useEffect, useRef, useState } from "react";
import { requestError } from "./use-idempotent-post";

type Calls = { chat: number; embedding: number; tool: number };
type Evidence = { id: number; document_id: string; ordinal: number; title: string; source: string; text: string };
type Quote = {
  request_id: string; conversation_id: string; revision: number; digest: string; status: string;
  currency: string; amount_micro: string; calls: Calls; expires_at_unix_ms: number;
  searches: { query: string; limit: number }[] | null;
  evidence?: Evidence[]; answer?: { insufficient_evidence: boolean; answer: string; citations: number[] } | null;
};
type Item = { planning: Quote; execution: Quote | null };
type History = { enabled: boolean; items: Item[] };
type Mutation = { action: string; id: string; body: Record<string, unknown> };
type Confirmation = { action: "approve-planning" | "approve-execution"; quote: Quote };
const statuses: Record<string, string> = {
  draft: "等待费用确认", queued: "等待执行", dispatching: "正在规划", running: "正在检索或回答",
  succeeded: "已完成", insufficient_evidence: "证据不足", failed: "执行失败", unknown: "结果未知",
  cancelled: "已取消", expired: "已过期", stale: "消息版本已变化",
};
const active = (q: Quote) => ["queued", "dispatching", "running"].includes(q.status);
const cancellable = (q: Quote) => q.status === "draft" || active(q);
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i;
function validQuote(value: unknown, conversation: string): value is Quote {
  const q = value as Quote | null;
  return !!q && uuid.test(q.request_id) && q.conversation_id === conversation && Number.isSafeInteger(q.revision) && q.revision > 0
    && /^[a-f0-9]{64}$/.test(q.digest) && Object.hasOwn(statuses, q.status) && q.currency === "USD"
    && typeof q.amount_micro === "string" && /^[1-9][0-9]{0,18}$/.test(q.amount_micro) && BigInt(q.amount_micro) <= BigInt("9223372036854775807")
    && !!q.calls && [q.calls.chat, q.calls.embedding, q.calls.tool].every(n => Number.isInteger(n) && n >= 0 && n <= 3)
    && Number.isSafeInteger(q.expires_at_unix_ms)
    && (q.searches === null || (Array.isArray(q.searches) && q.searches.length <= 3 && q.searches.every(s => typeof s.query === "string" && Number.isInteger(s.limit) && s.limit >= 1 && s.limit <= 5)))
    && (q.evidence === undefined || (Array.isArray(q.evidence) && q.evidence.length <= 15 && q.evidence.every((e, i) => e.id === i + 1 && uuid.test(e.document_id) && Number.isSafeInteger(e.ordinal) && e.ordinal >= 0 && [e.title, e.source, e.text].every(t => typeof t === "string"))))
    && (q.answer == null || (typeof q.answer.insufficient_evidence === "boolean" && typeof q.answer.answer === "string" && Array.isArray(q.answer.citations) && q.answer.citations.every(id => Number.isInteger(id) && id > 0 && id <= (q.evidence?.length ?? 0))));
}
function money(q: Quote) {
  const amount = BigInt(q.amount_micro);
  return `${q.currency} ${amount / BigInt(1000000)}.${(amount % BigInt(1000000)).toString().padStart(6, "0")}`;
}
function errorMessage(status: number) {
  return status === 409 ? "报价已失效、消息已变化或预算不足。请刷新历史核对；重新报价需要新规划请求。" : requestError(status);
}
export function ModelAgentPanel({ conversation, revision, disabled, onLock }: {
  conversation: string; revision: number | null; disabled: boolean; onLock: (value: boolean) => void;
}) {
  const [history, setHistory] = useState<History | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [loading, setLoading] = useState(true);
  const [readError, setReadError] = useState("");
  const [writeError, setWriteError] = useState("");
  const [pending, setPending] = useState<Mutation | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [discard, setDiscard] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const reader = useRef<AbortController | null>(null);
  const writer = useRef<AbortController | null>(null);
  const path = `/api/conversations/${conversation}/model-agents`;
  const locked = busy || !!pending || !!confirmation;
  useEffect(() => { onLock(locked); return () => onLock(false); }, [onLock, locked]);
  useEffect(() => () => { writer.current?.abort(); writer.current = null; }, []);
  function reload() { reader.current?.abort(); setLoading(true); setRefresh(v => v + 1); }
  useEffect(() => {
    const controller = new AbortController(); reader.current = controller;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function load() {
      try {
        const response = await fetch(path, { cache: "no-store", signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]) });
        if (!response.ok) throw new Error(errorMessage(response.status));
        const data: History = await response.json();
        if (typeof data?.enabled !== "boolean" || !Array.isArray(data.items) || data.items.length > 20
          || !data.items.every(i => validQuote(i.planning, conversation) && (i.execution === null || (validQuote(i.execution, conversation) && i.execution.request_id === i.planning.request_id)))
          || new Set(data.items.map(i => i.planning.request_id)).size !== data.items.length) throw new Error("服务返回无效报价，已停止授权，请刷新核对。");
        if (controller.signal.aborted) return;
        setHistory(data); setReadError(""); setNow(Date.now());
        if (data.items.some(i => active(i.planning) || (i.execution && active(i.execution)))) timer = setTimeout(() => void load(), 2000);
      } catch (e) {
        if (!controller.signal.aborted) setReadError(`${e instanceof Error && e.name === "Error" ? e.message : "无法读取模型助手历史。"} 已停止自动刷新。`);
      } finally { if (!controller.signal.aborted) setLoading(false); }
    }
    void load();
    return () => { controller.abort(); clearTimeout(timer); if (reader.current === controller) reader.current = null; };
  }, [path, conversation, refresh]);
  useEffect(() => {
    const deadlines = history?.items.flatMap(i => [i.planning, i.execution]).filter((q): q is Quote => !!q && q.status === "draft" && q.expires_at_unix_ms > now).map(q => q.expires_at_unix_ms) ?? [];
    if (!deadlines.length) return;
    const timer = setTimeout(() => setNow(Date.now()), Math.max(1, Math.min(...deadlines) - Date.now()));
    return () => clearTimeout(timer);
  }, [history, now]);
  const available = history?.enabled && !loading && !readError && !disabled;
  function canApprove(q: Quote, time = now) { return available && q.status === "draft" && q.revision === revision && q.expires_at_unix_ms > time && !busy && !pending; }
  async function send(mutation: Mutation) {
    if (writer.current || disabled) return;
    reader.current?.abort(); setLoading(false);
    const controller = new AbortController(); writer.current = controller;
    setPending(mutation); setBusy(true); setConfirmation(null); setWriteError(""); setDiscard(false);
    try {
      const response = await fetch(mutation.action === "create" ? path : `${path}/${mutation.id}/${mutation.action}`, {
        method: "POST", cache: "no-store", headers: { "Content-Type": "application/json", "X-Requested-With": "personal-ai" },
        body: JSON.stringify(mutation.body), signal: AbortSignal.any([controller.signal, AbortSignal.timeout(10000)]),
      });
      if (!response.ok) throw new Error(errorMessage(response.status));
      const result: unknown = await response.json();
      if (!validQuote(result, conversation) || result.request_id !== mutation.id
        || (mutation.action === "create" && result.revision !== mutation.body.expected_revision)
        || (mutation.action.startsWith("approve") && (result.digest !== mutation.body.digest || result.amount_micro !== mutation.body.accepted_amount_micro || result.status === "draft"))) throw new Error("操作结果不匹配，请刷新历史核对并保留原请求。");
      if (writer.current !== controller) return;
      setPending(null); reload();
    } catch (e) {
      if (writer.current === controller) setWriteError(e instanceof Error && e.name === "Error" ? e.message : "操作结果未确认，请刷新历史或重试原请求。");
    } finally { if (writer.current === controller) { writer.current = null; setBusy(false); } }
  }
  function review(action: Confirmation["action"], quote: Quote) { setAcknowledged(false); setConfirmation({ action, quote }); }
  function quoteCard(q: Quote, phase: "planning" | "execution") {
    const expired = q.status === "draft" && q.expires_at_unix_ms <= now;
    return <div className="modelAgentStage">
      <h5>{phase === "planning" ? "第一阶段：生成检索建议" : "第二阶段：检索并回答"} · {statuses[expired ? "expired" : q.status]}</h5>
      <p>本阶段最多预留 {money(q)}；聊天 {q.calls.chat} 次，向量化 {q.calls.embedding} 次，检索工具 {q.calls.tool} 次。</p>
      <p>消息版本 {q.revision}{q.revision !== revision ? "（与当前消息版本不一致）" : ""}</p>
      {q.status === "draft" && <p>报价截止：{new Date(q.expires_at_unix_ms).toLocaleString("zh-CN")}</p>}
      {q.searches && <ol>{q.searches.map((s, i) => <li key={i}><pre>{s.query}</pre><span>最多 {s.limit} 个片段</span></li>)}</ol>}
      {q.answer && <div>{q.answer.insufficient_evidence ? <p>当前资料不足以回答。未派发的回答预算已释放；若模型已调用，则按该次调用结算。</p> : <><p className="modelAgentAnswer">{q.answer.answer}</p><p>引用：{q.answer.citations.map(id => `[${id}]`).join(" ")}</p></>}</div>}
      {q.evidence?.map(e => <details key={e.id}><summary>[{e.id}] {e.title} · 查看原文</summary><p>来源：{e.source} · 第 {e.ordinal + 1} 块</p><pre>{e.text}</pre></details>)}
      {q.status === "unknown" && <p>结果未知，已派发调用的预算保留，不会自动重发。</p>}
      {q.status === "draft" && <button disabled={!canApprove(q) || !!confirmation} onClick={() => review(phase === "planning" ? "approve-planning" : "approve-execution", q)}>审阅{phase === "planning" ? "规划" : "检索回答"}费用</button>}
      {cancellable(q) && <button disabled={disabled || locked} onClick={() => void send({ action: `cancel-${phase}`, id: q.request_id, body: {} })}>取消{phase === "planning" ? "规划" : "检索回答"}</button>}
    </div>;
  }
  const selected = confirmation && history?.items.find(i => i.planning.request_id === confirmation.quote.request_id);
  const current = confirmation?.action === "approve-planning" ? selected?.planning : selected?.execution;
  const same = current && confirmation && current.digest === confirmation.quote.digest && current.amount_micro === confirmation.quote.amount_micro;
  return <section className="modelAgentPanel" aria-label="模型知识助手">
    <h4>模型知识助手</h4>
    <p>先确认规划费用，查看建议后再单独确认检索与回答费用。保存报价不调用模型，第一阶段费用不会因取消第二阶段退回。</p>
    <p>取消可阻止后续步骤，已派发调用可能继续计费。关闭页面或退出登录不会撤销已授权任务。</p>
    <button disabled={loading || busy} onClick={reload}>刷新模型助手历史</button>
    {loading && <p role="status">正在读取模型助手…</p>}
    {readError && <p role="alert">{readError}</p>}
    {history && !history.enabled && <p>管理员尚未启用模型助手，或当前配置已停用；历史仍可查询和取消。</p>}
    <button disabled={!available || locked || revision === null || revision < 1 || (history?.items.length ?? 0) >= 20} onClick={() => {
      if (revision === null) return;
      const id = crypto.randomUUID(); void send({ action: "create", id, body: { request_id: id, expected_revision: revision } });
    }}>生成规划报价（不调用模型）</button>
    <ol>{history?.items.slice().reverse().map(item => <li key={item.planning.request_id}>
      <small>请求 ID：{item.planning.request_id}</small>
      {quoteCard(item.planning, "planning")}
      {item.execution ? quoteCard(item.execution, "execution") : item.planning.status === "succeeded" && <button disabled={!available || locked || item.planning.revision !== revision} onClick={() => void send({ action: "preview-execution", id: item.planning.request_id, body: {} })}>预览检索回答报价（不调用模型）</button>}
    </li>)}</ol>
    {confirmation && <div className="pendingRequest" role="group" aria-label="确认模型阶段费用">
      <h5>确认{confirmation.action === "approve-planning" ? "规划" : "检索回答"}授权</h5>
      <p>本次最多预留 {money(confirmation.quote)}；聊天 {confirmation.quote.calls.chat} 次，向量化 {confirmation.quote.calls.embedding} 次，检索 {confirmation.quote.calls.tool} 次。</p>
      <p>此授权仅用于当前阶段，绑定消息版本 {confirmation.quote.revision} 及当前报价。未知结果可能保留全部预留。</p>
      <small>报价指纹：{confirmation.quote.digest}</small>
      <label className="agentConsent"><input type="checkbox" checked={acknowledged} onChange={e => setAcknowledged(e.target.checked)} />我确认本阶段金额上限与调用次数</label>
      <button disabled={!acknowledged || !same || !current || !canApprove(current)} onClick={() => {
        if (!acknowledged || !same || !current || !canApprove(current, Date.now())) return;
        const q = confirmation.quote;
        void send({ action: confirmation.action, id: q.request_id, body: { digest: q.digest, accepted_currency: q.currency, accepted_amount_micro: q.amount_micro, accepted_calls: { ...q.calls }, acknowledge_cost: true } });
      }}>确认金额并执行本阶段</button>
      <button onClick={() => setConfirmation(null)}>返回模型助手</button>
    </div>}
    {busy && <p role="status">正在提交模型助手请求…</p>}
    {pending && !busy && <div className="pendingRequest">
      {writeError && <p role="alert">{writeError}</p>}
      <p>结果未确认，原请求 ID：{pending.id}。请先刷新历史核对；重试会保留原金额、指纹及请求 ID，不创建新任务。</p>
      <button disabled={disabled} onClick={() => void send(pending)}>重试模型助手原请求</button>
      {!discard ? <button onClick={() => setDiscard(true)}>放弃模型助手本地请求</button> : <><p>放弃只清除本地请求，不取消后台任务；新请求可能再次计费。</p><button onClick={() => { setPending(null); setWriteError(""); setDiscard(false); reload(); }}>确认放弃模型助手本地请求</button><button onClick={() => setDiscard(false)}>保留原请求</button></>}
    </div>}
  </section>;
}
