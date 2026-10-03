"use client";
import { LearningModelAdvice } from "./learning-model-advice";
import { useEffect, useState } from "react";
import { type ModelAuthorization, type ModelAuthorizationPage, type ModelPreviewLoader, date } from "./learning-types";
const root = "/api/learning/model-authorizations";
const labels = { draft: "待确认", authorized: "已保存授权，尚未执行", running: "正在执行一次核验", succeeded: "核验建议已保存", unknown: "结果未知，不会自动重发", cancelled: "已取消", expired: "已到期", invalidated: "来源或连接已失效" };
export function LearningAuthorizationDetail({ item, locked, load, accept, openPlan }: { openPlan?: (path: string) => void; item: ModelAuthorization; locked: boolean; load: ModelPreviewLoader; accept: (v: ModelAuthorization) => void }) {
  const [sharing, setSharing] = useState(false); const [usage, setUsage] = useState(false);
  const [elapsed, setElapsed] = useState(() => Number(item.expires_at_unix_ms) <= Date.now());
  const [pending, setPending] = useState<{ path: string; body: object } | null>(null);
  useEffect(() => { const timer = setTimeout(() => setElapsed(true), Math.max(0, Number(item.expires_at_unix_ms) - Date.now())); return () => clearTimeout(timer); }, [item.expires_at_unix_ms]);
  const expired = elapsed;
  const active = ["draft", "authorized"].includes(item.status) && !expired;
  function receive(v: ModelAuthorization) { setPending(null); setSharing(false); setUsage(false); accept(v); }
  function send(operation: { path: string; body: object }) { setPending(operation); setSharing(false); setUsage(false); load<ModelAuthorization>(operation.path, receive, { method: "POST", body: operation.body }); }
  return <section aria-label="模型核验授权详情">
    <p>{expired && ["draft", "authorized"].includes(item.status) ? "已到期，请核对服务器状态" : labels[item.status]}</p>
    <p>连接 {item.connection_id}（版本 {item.connection_revision}），模型 {item.model}。有效至 {date(item.expires_at_unix_ms)}。</p>
    <p>保存授权后，可在已登录的本机显式执行一次，届时会消耗订阅额度或账户允许的 credits。批准不保证额度或模型可用，不会自动生成自评。</p>
    {(active || item.status === "running" || item.status === "succeeded") && item.preview && !pending && <details><summary>核对本次完整分享内容</summary><pre className="feedText" style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{JSON.stringify(item.preview, null, 2)}</pre></details>}
    {active && item.status === "draft" && !pending && <div>
      <label><input type="checkbox" checked={sharing} disabled={locked} onChange={e => setSharing(e.target.checked)} />同意将本次预览中的证据分享给所选模型</label>
      <label><input type="checkbox" checked={usage} disabled={locked} onChange={e => setUsage(e.target.checked)} />同意一次调用消耗所选连接的订阅额度</label>
      <button disabled={locked || !sharing || !usage} onClick={() => send({ path: `${root}/${item.request_id}/approve`, body: { digest: item.digest, acknowledge_sharing: sharing, acknowledge_subscription_usage: usage } })}>确认保存本次模型授权</button>
    </div>}
    <button disabled={locked} onClick={() => { setSharing(false); setUsage(false); load<ModelAuthorization>(`${root}/${item.request_id}`, receive); }}>核对模型授权状态</button>
    {(active || item.status === "running" || item.status === "succeeded") && !pending && <button disabled={locked} onClick={() => send({ path: `${root}/${item.request_id}/cancel`, body: {} })}>取消本次模型授权</button>}
    {item.status === "running" && <p>取消会阻止保存晚到建议；已经发出的请求仍可能消耗额度。请手动核对状态，页面不会重新发送模型请求。</p>}
    {item.status === "unknown" && <p>可能已经发送，无法确认结果。原授权不会再次执行；若要重新尝试，请重新预览并明确授权。</p>}
    {item.status === "succeeded" && item.advice && !pending && <><LearningModelAdvice advice={item.advice} />{openPlan && <button disabled={locked} onClick={() => openPlan(`/api/learning/plans/${item.plan_id}`)}>回到原训练证据</button>}</>}
    {pending && <div><p>请求结果尚未核对。可查询状态，或重试同一操作；不会创建新授权。</p><button disabled={locked} onClick={() => send(pending)}>重试原模型授权操作</button></div>}
  </section>;
}
export function LearningAuthorizationHistory({ locked, load, openPlan }: { openPlan: (path: string) => void; locked: boolean; load: ModelPreviewLoader }) {
  const [page, setPage] = useState<ModelAuthorizationPage | null>(null); const [item, setItem] = useState<ModelAuthorization | null>(null);
  function list(after?: string) { setItem(null); setPage(null); load<ModelAuthorizationPage>(`${root}${after ? `?after=${encodeURIComponent(after)}` : ""}`, setPage); }
  return <section aria-label="模型核验授权历史"><h3>模型核验授权历史</h3>
    <button disabled={locked} onClick={() => list()}>读取模型授权历史</button>
    {page && !page.items.length && <p>暂无模型核验授权。</p>}
    {page && <ul>{page.items.map(v => <li key={v.request_id}>{v.model} · {labels[v.status]} · {date(v.created_at_unix_ms)} <button disabled={locked} onClick={() => { setItem(null); load<ModelAuthorization>(`${root}/${v.request_id}`, setItem); }}>查看模型授权</button></li>)}</ul>}
    {page?.next_cursor && <button disabled={locked} onClick={() => list(page.next_cursor!)}>下一页模型授权</button>}
    {item && <LearningAuthorizationDetail key={`${item.request_id}:${item.status}`} item={item} locked={locked} load={load} openPlan={openPlan} accept={v => { setItem(v); setPage(null); }} />}
  </section>;
}
