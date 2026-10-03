"use client";

import { useState } from "react";
import { date, type ValueReading } from "./feed-value-types";

function safeLink(value: string | null) {
  try {
    const url = new URL(value ?? "");
    return ["https:", "http:"].includes(url.protocol) && !url.username && !url.password ? url.href : undefined;
  } catch { return undefined; }
}
export function FeedValueReading({ value, disabled }: { value: ValueReading; disabled: boolean }) {
  const [order, setOrder] = useState("model");
  const items = [...value.items].sort((a, b) => order === "rule" ? a.id - b.id : (b.model_score ?? -1) - (a.model_score ?? -1) || a.id - b.id);
  return <section aria-label="评分阅读视图">
    <h4>阅读评分条目</h4>
    <p>快照时间：{date(value.as_of_unix_ms)}。关键词：{value.keywords.join("、")}。</p>
    <p>显示原评分时的候选，未重新采集或调用模型。模型建议与规则分数含义不同，不直接比较数值；无法评分不等于零分。</p>
    <label>阅读排序<select aria-label="阅读排序" value={order} disabled={disabled} onChange={e => setOrder(e.target.value)}><option value="model">模型建议顺序</option><option value="rule">原规则顺序</option></select></label>
    <ol className="feedList" aria-label="评分阅读条目">{items.map(item => <li key={item.id}>
      <h5>{item.title || "无标题条目"}</h5>
      <p className="feedValueSummary">{item.summary || "原条目没有摘要。"}</p>
      <p>模型建议：{item.model_score === null ? "无法评分" : `${item.model_score} / 100`}；规则分数：{item.rule_score}；原规则名次：{item.id}</p>
      <p>{item.reason}</p>
      {safeLink(item.link) ? <a href={safeLink(item.link)} target="_blank" rel="noopener noreferrer" referrerPolicy="no-referrer">打开原文</a> : <small>没有可安全打开的原文链接。</small>}
    </li>)}</ol>
  </section>;
}
