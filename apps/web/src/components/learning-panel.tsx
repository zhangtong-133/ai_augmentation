"use client";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { type ModelReviewPreview, type Snapshot, type Skill, type History, type SavedPlan, type Operation, statuses, date } from "./learning-types";
import { LearningProgressView, type LearningProgress } from "./learning-progress";
import { LearningPlanView } from "./learning-plan-view";
const root = "/api/learning";
const empty = (): History => ({ items: [], next_cursor: null });
function message(status: number) {
  if (status === 401) return "登录已失效，请重新登录。";
  if (status === 404) return "记录不存在或当前账户无权访问。";
  if (status === 409) return "版本已变化、训练已有结果或已达到额度。请核对保存状态，再刷新学习数据。";
  if ([400, 413, 422].includes(status)) return "输入不符合要求：请检查名称、前置技能是否成环、分数及分钟范围。";
  return "服务暂不可用，操作可能已保存，请核对或重试原操作。";
}
class LearningHttpError extends Error {
  constructor(readonly status: number) { super(message(status)); }
}
export function LearningPanel() {
  const [progress, setProgress] = useState<LearningProgress | null>(null);
  const [progressError, setProgressError] = useState("");
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [history, setHistory] = useState<History>(empty);
  const [pages, setPages] = useState<string[]>([]);
  const [selected, setSelected] = useState<SavedPlan | null>(null);
  const [editing, setEditing] = useState<Skill | null>(null);
  const [name, setName] = useState(""); const [enabled, setEnabled] = useState(true); const [parents, setParents] = useState<string[]>([]);
  const [ratingSkill, setRatingSkill] = useState(""); const [score, setScore] = useState("50");
  const [goals, setGoals] = useState<string[]>([]); const [budget, setBudget] = useState("30");
  const [deleting, setDeleting] = useState<Skill | null>(null);
  const [pending, setPending] = useState<Operation | null>(null);
  const [busy, setBusy] = useState(false); const [ready, setReady] = useState(false); const [expired, setExpired] = useState(false);
  const [error, setError] = useState(""); const [notice, setNotice] = useState("");
  const active = useRef<AbortController | null>(null); const alive = useRef(true);
  const valid = (c: AbortController) => alive.current && active.current === c && !c.signal.aborted;
  function resetForms() { setEditing(null); setName(""); setEnabled(true); setParents([]); setRatingSkill(""); setScore("50"); setGoals([]); setBudget("30"); setDeleting(null); }
  async function request<T>(path: string, c: AbortController, operation?: Operation): Promise<T> {
    const response = await fetch(path, { method: operation?.method ?? "GET", cache: "no-store", headers: operation ? { "Content-Type": "application/json", "X-Requested-With": "personal-ai" } : undefined, body: operation ? JSON.stringify(operation.body) : undefined, signal: AbortSignal.any([c.signal, AbortSignal.timeout(10000)]) });
    if (!response.ok) {
      if (response.status === 401 && valid(c)) { setExpired(true); setReady(false); setProgress(null); setProgressError(""); setSnapshot(null); setHistory(empty()); setPages([]); setSelected(null); setPending(null); setNotice(""); resetForms(); }
      throw new LearningHttpError(response.status);
    }
    return response.json();
  }
  async function run(work: (c: AbortController) => Promise<void>) {
    if (active.current) return;
    const c = new AbortController(); active.current = c; setBusy(true); setError("");
    try { await work(c); } catch (e) { if (valid(c)) setError(e instanceof Error && e.name === "Error" ? e.message : "连接中断，结果尚未确认。请核对或重试原操作。"); }
    finally { if (valid(c)) { active.current = null; setBusy(false); } }
  }
  async function list(c: AbortController, cursors: string[]) {
    const data = await request<History>(`${root}/plans${cursors.length ? `?after=${encodeURIComponent(cursors.at(-1)!)}` : ""}`, c);
    if (valid(c)) { setHistory(data); setPages(cursors); }
  }
  async function loadProgress(c: AbortController) {
    setProgress(null); setProgressError("");
    try {
      const data = await request<LearningProgress>(`${root}/progress`, c);
      if (valid(c)) setProgress(data);
    } catch (e) {
      if (e instanceof LearningHttpError && e.status === 401) throw e;
      if (valid(c)) setProgressError("学习进度暂不可用，请刷新重试。管理功能仍可使用。");
    }
  }
  async function load(c: AbortController) {
    setSelected(null); setReady(false); setProgress(null); setProgressError(""); resetForms();
    const data = await request<Snapshot>(`${root}/snapshot`, c);
    await list(c, []);
    if (valid(c)) { setSnapshot(data); setReady(true); await loadProgress(c); }
  }
  useEffect(() => {
    alive.current = true; const timer = window.setTimeout(() => void run(load), 0);
    return () => { window.clearTimeout(timer); alive.current = false; active.current?.abort(); active.current = null; };
    // AccountPanel uses the authenticated user ID as this component's key.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  async function mutate(operation: Operation) {
    await run(async c => {
      setPending(operation); setNotice(""); setDeleting(null); setSelected(null); setProgress(null); setProgressError("");
      const result = await request<SavedPlan>(operation.path, c, operation);
      if (!valid(c)) return;
      setPending(null); setNotice("学习操作已保存。");
      await load(c);
      if (valid(c) && operation.showPlan) setSelected(result);
    });
  }
  function inspect(path: string, recover = false) {
    void run(async c => {
      setSelected(null);
      if (path === `${root}/snapshot`) await load(c);
      else { const result = await request<SavedPlan>(path, c); if (valid(c)) { setSelected(result); await loadProgress(c); } }
      if (recover && valid(c)) { setPending(null); setNotice("已查询当前保存状态，请核对内容。查询不会提交新操作。"); }
    });
  }
  function edit(skill: Skill | null) { setEditing(skill); setName(skill?.name ?? ""); setEnabled(skill?.enabled ?? true); setParents(skill?.prerequisite_ids ?? []); setDeleting(null); }
  function save(e: FormEvent) {
    e.preventDefault(); if (locked) return;
    void mutate({ method: "PUT", path: `${root}/skills/${editing?.skill_id ?? crypto.randomUUID()}`, lookup: `${root}/snapshot`, body: { revision: editing?.revision ?? "0", name, enabled, prerequisite_ids: parents } });
  }
  const locked = busy || !ready || expired || pending !== null;
  const skills = snapshot?.skills.filter(s => !s.deleted) ?? [];
  function rating(skill: Skill) {
    return snapshot?.assessments.filter(a => a.skill_id === skill.skill_id && a.skill_revision === skill.revision).reduce<Snapshot["assessments"][number] | undefined>((latest, a) => !latest || BigInt(a.assessed_at_unix_ms) > BigInt(latest.assessed_at_unix_ms) ? a : latest, undefined);
  }
  return <section className="feedPanel briefPanel learningPanel" aria-label="学习管理">
    <h2>学习管理</h2><p>用技能前置关系和自己的评分安排练习。评分是自我记录，不代表客观能力；完成训练不会自动加分。</p>
    <button disabled={busy || expired} onClick={() => void run(load)}>刷新学习数据</button>
    {busy && <p role="status">正在处理学习数据…</p>}{error && <p role="alert">{error}</p>}{notice && <p role="status">{notice}</p>}
    {pending && <div className="feedReview"><p>操作结果未确认，请核对或重试原操作。</p><button disabled={busy || expired} onClick={() => inspect(pending.lookup, true)}>核对原学习操作</button><button disabled={busy || expired} onClick={() => void mutate(pending)}>重试原学习操作</button><button disabled={busy || expired} onClick={() => { setPending(null); setNotice("已关闭核对提示，已保存的内容仍可查询。"); }}>关闭学习核对提示</button></div>}
    <LearningProgressView data={progress} error={progressError} busy={busy} expired={expired} refresh={() => void run(loadProgress)} />
    <h3>技能与前置关系</h3><p>最多 100 项技能（含已删除），每项最多 8 个前置技能。修改技能后需重新自评，旧评分不用于新版本。</p>
    <form onSubmit={save}><h4>{editing ? "修改技能" : "添加技能"}</h4>
      <label>技能名称<input required maxLength={120} value={name} disabled={locked} onChange={e => setName(e.target.value)} /></label>
      <label><input type="checkbox" checked={enabled} disabled={locked} onChange={e => setEnabled(e.target.checked)} />启用技能</label>
      <fieldset disabled={locked}><legend>前置技能（最多 8 项）</legend>{(snapshot?.skills ?? []).filter(s => s.skill_id !== editing?.skill_id && (!s.deleted || parents.includes(s.skill_id))).map(s => <label key={s.skill_id}><input type="checkbox" checked={parents.includes(s.skill_id)} disabled={!parents.includes(s.skill_id) && parents.length >= 8} onChange={e => setParents(e.target.checked ? [...parents, s.skill_id] : parents.filter(id => id !== s.skill_id))} />{s.name}{!s.enabled && "（不可用）"}</label>)}</fieldset>
      <button disabled={locked || !name.trim()}>保存技能</button>{editing && <button type="button" disabled={locked} onClick={() => edit(null)}>放弃修改技能</button>}
    </form>
    {ready && !skills.length && <p>暂无技能，先添加一项想学习的技能。</p>}
    <ul className="feedList" aria-label="技能列表">{skills.map(s => <li key={s.skill_id}><h4>{s.name}</h4><p>{s.enabled ? "已启用" : "已停用"} · 版本 {s.revision} · {rating(s) ? `当前自评 ${rating(s)!.score} 分` : "当前版本尚未自评"}</p><p>前置：{s.prerequisite_ids.map(id => snapshot?.skills.find(p => p.skill_id === id)?.name ?? "不可用技能").join("、") || "无"}</p><button disabled={locked} onClick={() => edit(s)}>修改 {s.name}</button><button disabled={locked} onClick={() => setDeleting(s)}>删除 {s.name}</button></li>)}</ul>
    {deleting && <div className="feedReview"><p>确认删除技能“{deleting.name}”？相关自评、计划正文和训练结果将被清除，依赖它的技能会受阻。</p><button disabled={locked} onClick={() => void mutate({ path: `${root}/skills/${deleting.skill_id}`, lookup: `${root}/snapshot`, method: "DELETE", body: { revision: deleting.revision } })}>确认删除技能</button><button disabled={locked} onClick={() => setDeleting(null)}>保留技能</button></div>}
    <h3>显式自评</h3><p>0–100 分，70 分是本版本规划器的参考目标。每个账户最多保存 1000 次自评（含清除记录）。</p>
    <form onSubmit={e => { e.preventDefault(); const skill = skills.find(s => s.skill_id === ratingSkill); if (!skill || !snapshot || locked) return; void mutate({ path: `${root}/assessments`, lookup: `${root}/snapshot`, method: "POST", body: { request_id: crypto.randomUUID(), expected_revision: snapshot.revision, skill_id: skill.skill_id, skill_revision: skill.revision, score: Number(score) } }); }}>
      <label>自评技能<select required value={ratingSkill} disabled={locked} onChange={e => setRatingSkill(e.target.value)}><option value="">请选择技能</option>{skills.filter(s => s.enabled).map(s => <option key={s.skill_id} value={s.skill_id}>{s.name}</option>)}</select></label>
      <label>自评分数<input type="number" min={0} max={100} step={1} required value={score} disabled={locked} onChange={e => setScore(e.target.value)} /></label><button disabled={locked || !ratingSkill}>保存自评</button>
    </form>
    <h3>生成学习计划</h3><p>选择最多 10 项目标，预算 5–180 分钟，每份最多安排 5 项训练。每天（UTC）最多生成 10 份，共 1000 份，删除不恢复额度。</p>
    <form onSubmit={e => { e.preventDefault(); if (!snapshot || locked) return; const id = crypto.randomUUID(); void mutate({ path: `${root}/plans`, lookup: `${root}/plans/${id}`, method: "POST", showPlan: true, body: { request_id: id, expected_revision: snapshot.revision, budget_minutes: Number(budget), goal_skill_ids: goals } }); }}>
      <fieldset disabled={locked}><legend>学习目标</legend>{skills.filter(s => s.enabled).map(s => <label key={s.skill_id}><input type="checkbox" checked={goals.includes(s.skill_id)} disabled={!goals.includes(s.skill_id) && goals.length >= 10} onChange={e => setGoals(e.target.checked ? [...goals, s.skill_id] : goals.filter(id => id !== s.skill_id))} />{s.name}</label>)}</fieldset>
      <label>计划分钟<input type="number" min={5} max={180} step={1} required value={budget} disabled={locked} onChange={e => setBudget(e.target.value)} /></label><button disabled={locked || !goals.length}>生成学习计划</button>
    </form>
    <h3>学习计划历史</h3>{ready && !history.items.length && <p>暂无学习计划。</p>}
    <ul className="feedList" aria-label="学习计划历史">{history.items.map(p => <li key={p.request_id}>{date(p.created_at_unix_ms)} · {statuses[p.status]}<br /><button disabled={locked} onClick={() => inspect(`${root}/plans/${p.request_id}`)}>查看学习计划</button></li>)}</ul>
    <button disabled={locked || !pages.length} onClick={() => void run(c => list(c, pages.slice(0, -1)))}>上一页学习计划</button><button disabled={locked || !history.next_cursor} onClick={() => void run(c => list(c, [...pages, history.next_cursor!]))}>下一页学习计划</button>
    {selected && <LearningPlanView key={selected.request_id} saved={selected} skills={skills} snapshotRevision={snapshot?.revision} locked={locked} inspect={inspect} loadPreview={(path, accept) => void run(async c => { const data = await request<ModelReviewPreview>(path, c); if (valid(c)) accept(data); })} submit={o => void mutate(o)} />}
  </section>;
}
