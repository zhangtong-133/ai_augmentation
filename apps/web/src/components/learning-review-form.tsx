"use client";
import { useState } from "react";
import { type Operation, type ReviewBody, type TrainingReview } from "./learning-types";
const fields: { key: keyof ReviewBody; label: string }[] = [
  { key: "explanation", label: "概念解释" }, { key: "work", label: "独立练习" },
  { key: "verification", label: "结果验证" }, { key: "limitations", label: "局限与反例" },
];
const verdicts = { missing: "缺少证据", unverified: "尚待核验", supported: "我认为有证据支持" };
const initial = (): ReviewBody => Object.fromEntries(fields.map(({ key }) => [key, { verdict: "unverified", reason: "" }])) as ReviewBody;
export function LearningReviewForm({ review, evidenceId, path, lookup, locked, current, snapshotRevision, submit }: {
  review: TrainingReview | null | undefined; evidenceId: string; path: string; lookup: string;
  locked: boolean; current: boolean; snapshotRevision?: string; submit: (o: Operation) => void;
}) {
  const [body, setBody] = useState<ReviewBody>(initial);
  const [score, setScore] = useState("");
  const [confirm, setConfirm] = useState(false);
  const submitBody = () => submit({ path, lookup, method: "POST", showPlan: true, body: { request_id: crypto.randomUUID(), evidence_request_id: evidenceId, body } });
  if (review?.status === "invalidated") return <p>核验已失效，正文及关联自评已清除。</p>;
  if (review?.body) {
    const supported = fields.every(({ key }) => review.body![key].verdict === "supported");
    return <section aria-label="已保存的用户核验"><h5>用户核验记录</h5><p>这是你对材料的判断，不是模型评估或客观能力认证。</p>
      {fields.map(({ key, label }) => <div key={key}><p><strong>{label}：{verdicts[review.body![key].verdict]}</strong></p><p className="feedText">{review.body![key].reason}</p></div>)}
      {review.status === "confirmed" ? <p>已确认自评 {review.confirmed_score} 分。此记录对应当时的技能版本；删除证据会撤销这条自评。</p> : !supported ? <p>仍有缺少证据或待核验项，不能据此确认分数。请在新的训练中补充材料，再重新核验。</p> : !current || !snapshotRevision ? <p>技能版本或计划来源不可用，不能确认这条核验。</p> : <div>
        <p>请自行填写 0–100 分自评。系统不推荐分数；确认将更新当前技能的自评，删除证据会撤销此条自评。</p>
        <label>核验后的自评分数<input type="number" min={0} max={100} step={1} value={score} disabled={locked} onChange={e => { setScore(e.target.value); setConfirm(false); }} /></label>
        <button disabled={locked || !/^\d+$/.test(score) || Number(score) > 100} onClick={() => setConfirm(true)}>准备确认自评</button>
        {confirm && <div><p>确认将你填写的 {score} 分保存为自评？核验记录和证据将与这条自评关联。</p>
          <button disabled={locked} onClick={() => submit({ path: `${path}/confirm`, lookup, method: "POST", showPlan: true, body: { request_id: crypto.randomUUID(), review_request_id: review.request_id, expected_revision: snapshotRevision, score: Number(score) } })}>确认记录自评</button>
          <button disabled={locked} onClick={() => setConfirm(false)}>返回调整分数</button>
        </div>}
      </div>}
    </section>;
  }
  if (!current) return <p>技能版本或计划来源不可用，不能为这份旧证据新增核验。</p>;
  return <details><summary>逐项核验证据</summary><p>请回看上方材料，逐项选择判断并填写理由（每项最多 500 字）。记录保存后不可修改；仅当四项都有证据支持时，才可另行确认自评分数。你可以暂不保存。</p>
    {fields.map(({ key, label }) => <fieldset key={key} disabled={locked}><legend>{label}</legend>
      <label>{label}判断<select value={body[key].verdict} onChange={e => { setBody({ ...body, [key]: { ...body[key], verdict: e.target.value as ReviewBody[typeof key]["verdict"] } }); setConfirm(false); }}>{Object.entries(verdicts).map(([value, text]) => <option key={value} value={value}>{text}</option>)}</select></label>
      <label>{label}核验理由<textarea rows={2} maxLength={500} value={body[key].reason} onChange={e => { setBody({ ...body, [key]: { ...body[key], reason: e.target.value } }); setConfirm(false); }} /></label>
    </fieldset>)}
    <button disabled={locked || fields.some(({ key }) => !body[key].reason.trim())} onClick={() => setConfirm(true)}>保存核验记录</button>
    {confirm && <div><p>确认保存上述判断与理由？这一步不会修改自评。</p><button disabled={locked} onClick={submitBody}>确认保存核验</button><button disabled={locked} onClick={() => setConfirm(false)}>返回修改核验</button></div>}
  </details>;
}
