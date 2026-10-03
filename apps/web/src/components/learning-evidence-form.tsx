"use client";
import { useState } from "react";
import { LearningReviewForm } from "./learning-review-form";
import { type TrainingEvidence, type EvidenceBody, type Operation, date } from "./learning-types";
const fields: { key: keyof EvidenceBody; label: string }[] = [
  { key: "explanation", label: "概念解释" },
  { key: "work", label: "练习产物与独立完成范围" },
  { key: "verification", label: "验证步骤与结果" },
  { key: "limitations", label: "局限与待验证问题" },
];
export function LearningEvidenceForm({ evidence, path, lookup, locked, current, snapshotRevision, submit }: {
  evidence: TrainingEvidence | null | undefined; path: string; lookup: string;
  locked: boolean; current: boolean; snapshotRevision?: string; submit: (o: Operation) => void;
}) {
  const [body, setBody] = useState<EvidenceBody>({ explanation: "", work: "", verification: "", limitations: "" });
  const [confirm, setConfirm] = useState<"save" | "delete" | null>(null);
  if (evidence?.deleted) return <p>结构化证据已删除，相关核验及其确认自评也已清除，旧请求不能恢复正文。需要补充时请创建新的训练。</p>;
  if (evidence?.body) return <section aria-label="已保存的结构化证据">
    <h5>结构化证据 · 待核验</h5><p>{date(evidence.created_at_unix_ms)}。这些材料由你填写，尚未验证，不代表能力提升。</p>
    {fields.map(({ key, label }) => <div key={key}><h6>{label}</h6><p className="feedText">{evidence.body![key]}</p></div>)}
    <LearningReviewForm review={evidence.review} evidenceId={evidence.request_id} path={`${path}/review`} lookup={lookup} locked={locked} current={current} snapshotRevision={snapshotRevision} submit={submit} />
    <button disabled={locked} onClick={() => setConfirm("delete")}>删除结构化证据</button>
    {confirm === "delete" && <div><p>确认清除这份证据的全部正文？关联核验正文和由它确认的自评也会清除；原始训练记录保留，删除后不能恢复。</p>
      <button disabled={locked} onClick={() => submit({ path, lookup, method: "DELETE", body: { request_id: evidence.request_id }, showPlan: true })}>确认删除证据</button>
      <button disabled={locked} onClick={() => setConfirm(null)}>保留证据</button>
    </div>}
  </section>;
  if (!current) return <p>技能版本或计划来源不可用，不能向旧任务补写证据。请基于当前技能生成新的训练。</p>;
  return <details><summary>补充结构化证据</summary><p>四项均必填，每项最多 2000 字。保存后不可修改，可删除；链接仅作为文本保存，不自动访问。</p>
    {fields.map(({ key, label }) => <label key={key}>{label}<textarea rows={3} maxLength={2000} disabled={locked} value={body[key]} onChange={e => { setBody({ ...body, [key]: e.target.value }); setConfirm(null); }} /></label>)}
    <button disabled={locked || fields.some(({ key }) => !body[key].trim())} onClick={() => setConfirm("save")}>保存结构化证据</button>
    {confirm === "save" && <div><p>确认保存并关联到这项训练及其技能版本？材料仍需核验，自评分数不会改变。</p>
      <button disabled={locked} onClick={() => submit({ path, lookup, method: "POST", body: { request_id: crypto.randomUUID(), body }, showPlan: true })}>确认保存证据</button>
      <button disabled={locked} onClick={() => setConfirm(null)}>返回修改证据</button>
    </div>}
  </details>;
}
