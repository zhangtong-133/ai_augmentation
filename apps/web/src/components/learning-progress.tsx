"use client";
import { date } from "./learning-types";
export type LearningProgress = {
  timezone: "UTC"; as_of_unix_ms: string; day_start_unix_ms: string; day_end_unix_ms: string; revision: string; target_score: number;
  enabled_skills: number; assessed_skills: number; target_reached_skills: number; ready_plans: number; historical_plans: number;
  pending_tasks: number; completed_tasks: number; cancelled_tasks: number; completed_today: number; cancelled_today: number; recorded_minutes_today: number;
};
export function LearningProgressView({ data, error, busy, expired, refresh }: { data: LearningProgress | null; error: string; busy: boolean; expired: boolean; refresh: () => void }) {
  return <section className="feedReview learningProgress" aria-label="学习进度">
    <h3>学习进度</h3>
    <p>今日按 UTC 00:00–24:00（北京时间 08:00 至次日 08:00）统计，截至下方更新时间。只统计仍保留的有效计划和训练结果；删除后相应记录不再计入。</p>
    {error && <p role="alert">{error}</p>}
    {!data && !error && <p>{expired ? "登录已失效，进度已清除。" : busy ? "正在读取学习进度…" : "暂无进度快照，请刷新。"}</p>}
    {data && <>
      <dl className="overviewMetrics">
        <div><dt>启用技能</dt><dd>{data.enabled_skills}</dd></div>
        <div><dt>当前版本已自评</dt><dd>{data.assessed_skills} / {data.enabled_skills}</dd></div>
        <div><dt>自评分数达到参考目标</dt><dd>{data.target_reached_skills}</dd></div>
        <div><dt>待记录训练</dt><dd>{data.pending_tasks}</dd></div>
        <div><dt>今日完成</dt><dd>{data.completed_today}</dd></div>
        <div><dt>今日取消</dt><dd>{data.cancelled_today}</dd></div>
        <div><dt>今日记录用时（分钟）</dt><dd>{data.recorded_minutes_today}</dd></div>
      </dl>
      <p>现存计划 {data.ready_plans} 份，其中历史快照 {data.historical_plans} 份。现存完成记录 {data.completed_tasks} 项，取消记录 {data.cancelled_tasks} 项。</p>
      <p>参考目标为自评 {data.target_score} 分，不代表客观能力或前置关系已满足。待记录训练包含历史计划；用时包含用户显式记录的练习与自评准备时间，完成记录不会自动改分。</p>
      {data.enabled_skills === 0 && <p>先添加并启用一项技能，再填写自评和安排学习计划。</p>}
      <p>更新于 {date(data.as_of_unix_ms)} · 学习版本 {data.revision}</p>
    </>}
    <button disabled={busy || expired} onClick={refresh}>刷新学习进度</button>
  </section>;
}
