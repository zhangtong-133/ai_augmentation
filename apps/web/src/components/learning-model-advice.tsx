"use client";
import { type ModelAdvice, type EvidenceBody } from "./learning-types";
const fields: { key: keyof EvidenceBody; label: string }[] = [{ key: "explanation", label: "概念解释" }, { key: "work", label: "独立练习" }, { key: "verification", label: "结果验证" }, { key: "limitations", label: "局限与反例" }];
const verdicts = { missing: "材料不足", unverified: "尚未验证", supported: "材料支持（模型建议）" };
export function LearningModelAdvice({ advice }: { advice: ModelAdvice }) {
  return <section aria-label="模型核验建议"><h4>模型核验建议 · 仍需人工复核</h4>
    <p>原文引用已通过匹配检查，这不证明模型判断正确，也不代表能力提升。请回到原训练逐项核验；自评分数仍由你主动填写并确认。</p>
    {fields.map(({ key, label }) => <div key={key}><h5>{label}：{verdicts[advice[key].verdict]}</h5><p className="feedText">{advice[key].reason}</p>
      {advice[key].citations.map((citation, i) => <blockquote key={i}><p>引用：{fields.find(f => f.key === citation.field)?.label}</p><p className="feedText">{citation.quote}</p></blockquote>)}
    </div>)}
  </section>;
}
