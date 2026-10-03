"use client";
import { useState } from "react";
import { LearningEvidenceForm } from "./learning-evidence-form";
import { LearningEvidenceReview } from "./learning-evidence-review";
import { type Skill, type Operation, type SavedPlan, type Task, statuses, states, date } from "./learning-types";
function ResultForm({ task, path, locked, submit }: { task: Task; path: string; locked: boolean; submit: (o: Operation) => void }) {
  const [note, setNote] = useState("");
  const [minutes, setMinutes] = useState(String(task.target_minutes));
  const [confirm, setConfirm] = useState<"completed" | "cancelled" | null>(null);
  return <div>
    <p>建议记录具体练习产物、独立完成的部分、验证步骤与结果，以及尚未掌握的问题。记录本身不证明能力提升。</p>
    <label>训练记录<textarea rows={2} maxLength={2000} value={note} disabled={locked} onChange={e => { setNote(e.target.value); setConfirm(null); }} /></label>
    <label>实际练习分钟<input type="number" min={1} max={180} step={1} value={minutes} disabled={locked} onChange={e => { setMinutes(e.target.value); setConfirm(null); }} /></label>
    <button disabled={locked || !/^\d+$/.test(minutes) || Number(minutes) < 1 || Number(minutes) > 180} onClick={() => setConfirm("completed")}>记录完成</button>
    <button disabled={locked} onClick={() => setConfirm("cancelled")}>取消这项训练</button>
    {confirm && <div><p>{confirm === "completed" ? "确认保存完成记录？自评分数不会改变，如需更新请单独提交自评。" : "确认取消？取消记录的分钟数为 0。"}结果保存后不能修改。</p>
      <button disabled={locked} onClick={() => submit({ path: `${path}/tasks/${task.task_id}/result`, lookup: path, method: "POST", showPlan: true, body: { request_id: crypto.randomUUID(), outcome: confirm, note, actual_minutes: confirm === "completed" ? Number(minutes) : 0 } })}>确认训练结果</button>
      <button disabled={locked} onClick={() => setConfirm(null)}>返回修改</button>
    </div>}
  </div>;
}
export function LearningPlanView({ saved, skills, snapshotRevision, locked, inspect, submit }: { snapshotRevision?: string; skills: Skill[]; saved: SavedPlan; locked: boolean; inspect: (p: string) => void; submit: (o: Operation) => void }) {
  const [deleting, setDeleting] = useState(false);
  const path = `/api/learning/plans/${saved.request_id}`;
  return <div className="feedReview" aria-label="学习计划详情"><h3>{statuses[saved.status]} · {date(saved.created_at_unix_ms)}</h3>
    <button disabled={locked} onClick={() => inspect(path)}>更新学习计划状态</button>
    {saved.plan ? <><p>计划快照版本 {saved.snapshot_revision}。预算 {saved.plan.budget_minutes} 分钟，剩余 {saved.plan.remaining_minutes} 分钟；另有 {saved.plan.unscheduled_ready_count} 项可训练技能未排入。</p>
      {saved.source_assessments_available === false && <p role="status">来源自评已撤销。本计划保留当时的分数和训练记录，不再接受新增训练、证据或确认；如需清除历史内容，请删除本计划。</p>}
      <p>这是生成时的技能与自评快照。技能修改后，可重新生成计划；删除任一来源技能将清除本计划正文及结果。</p>
      <ul className="feedList" aria-label="技能评估">{saved.plan.evaluations.map(e => <li key={e.skill_id}>{e.name}：{states[e.state] ?? e.state}{e.self_reported_score !== null && `，自评 ${e.self_reported_score} 分`}{e.blocking_skill_ids.length > 0 && <p>受阻于：{e.blocking_skill_ids.map(id => saved.plan!.evaluations.find(s => s.skill_id === id)?.name ?? "不可用的前置技能").join("、")}</p>}</li>)}</ul>
      {!saved.plan.tasks.length && <p>当前没有可安排的训练。请查看前置技能、自评或调整时间预算。</p>}
      <ol className="feedList" aria-label="训练任务">{saved.plan.tasks.map(task => {
        const result = saved.results.find(r => r.task_id === task.task_id);
        const review = saved.evidence_reviews?.find(r => r.task_id === task.task_id && r.skill_id === task.skill_id && r.skill_revision === task.skill_revision && r.result_request_id === (result?.request_id ?? null));
        const noteId = `training-note-${task.task_id}`;
        return <li key={task.task_id}><h4>{task.title}</h4><p>{task.instructions}</p><p>{task.kind === "self_assessment" ? "自评准备" : "练习"} · 建议 {task.target_minutes} 分钟 · 自评参考目标 {task.target_score} 分</p>
          {result ? <div id={noteId} tabIndex={-1}><p>{result.outcome === "completed" ? "已记录完成" : "已取消训练"} · {result.actual_minutes} 分钟 · {date(result.recorded_at_unix_ms)}</p><p className="feedText">{result.note || "未填写记录"}</p></div> : <ResultForm task={task} path={path} locked={locked || saved.source_assessments_available === false} submit={submit} />}
          {result?.outcome === "completed" && <LearningEvidenceForm evidence={result.evidence} snapshotRevision={snapshotRevision} path={`${path}/tasks/${task.task_id}/evidence`} lookup={path} locked={locked} current={saved.source_assessments_available !== false && skills.some(s => s.skill_id === task.skill_id && s.revision === task.skill_revision && s.enabled && !s.deleted)} submit={submit} />}
          {review && <LearningEvidenceReview review={review} noteId={noteId} />}
        </li>;
      })}</ol>
    </> : <p>正文及训练结果已清除，不能恢复。</p>}
    {saved.status !== "deleted" && <button disabled={locked} onClick={() => setDeleting(true)}>删除学习计划</button>}
    {deleting && <div><p>确认清除计划正文和所有训练结果？历史元数据保留，删除不恢复生成额度。</p><button disabled={locked} onClick={() => submit({ path, lookup: path, method: "DELETE", body: {} })}>确认删除学习计划</button><button disabled={locked} onClick={() => setDeleting(false)}>保留学习计划</button></div>}
  </div>;
}
