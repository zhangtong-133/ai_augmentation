export type Skill = { skill_id: string; revision: string; name: string; enabled: boolean; deleted: boolean; prerequisite_ids: string[] };
export type Snapshot = { revision: string; skills: Skill[]; assessments: { assessment_id: string; skill_id: string; skill_revision: string; score: number; assessed_at_unix_ms: string }[] };
export type Summary = { request_id: string; created_at_unix_ms: string; status: "ready" | "invalidated" | "deleted" };
export type History = { items: Summary[]; next_cursor: string | null };
export type Task = { task_id: string; skill_id: string; skill_revision: string; kind: "self_assessment" | "practice"; title: string; instructions: string; target_minutes: number; target_score: number };
export type EvidenceReview = {
  task_id: string; skill_id: string; skill_revision: string;
  result_request_id: string | null;
  state: "not_recorded" | "cancelled" | "missing_note" | "unverified";
};
export type EvidenceBody = { explanation: string; work: string; verification: string; limitations: string };
export type ReviewBody = Record<"explanation" | "work" | "verification" | "limitations", { verdict: "missing" | "unverified" | "supported"; reason: string }>;
export type TrainingReview = { request_id: string; evidence_request_id: string; rubric_version: string; body: ReviewBody | null; status: "pending" | "confirmed" | "invalidated"; confirmed_score: number | null };
export type TrainingEvidence = { review?: TrainingReview | null; request_id: string; created_at_unix_ms: string; deleted: boolean; body: EvidenceBody | null };
export type SavedPlan = Summary & {
  source_assessments_available?: boolean;
  evidence_reviews?: EvidenceReview[]; snapshot_revision: string; results: { evidence?: TrainingEvidence | null; task_id: string; request_id: string; outcome: "completed" | "cancelled"; note: string; actual_minutes: number; recorded_at_unix_ms: string }[]; plan: null | { budget_minutes: number; remaining_minutes: number; unscheduled_ready_count: number; evaluations: { skill_id: string; name: string; state: string; self_reported_score: number | null; blocking_skill_ids: string[] }[]; tasks: Task[] } };
export type Operation = { path: string; lookup: string; method: string; body: object; showPlan?: boolean };
export const statuses = { ready: "已生成", invalidated: "来源已删除，计划已失效", deleted: "已删除" };
export const states: Record<string, string> = { unavailable: "不可用", blocked: "前置技能未满足", needs_assessment: "待自评", needs_practice: "待练习", satisfied: "已达到自评目标" };
export function date(value: string) { const d = new Date(Number(value)); return Number.isFinite(d.getTime()) ? d.toISOString().slice(0, 16).replace("T", " ") + " UTC" : "日期不可显示"; }

export type ModelReviewPreview = { protocol_version: string; system_prompt: string; input: { input_digest: string; skill_name: string; task_instructions: string; evidence: EvidenceBody } };
export type ModelPreviewLoader = <T>(path: string, accept: (value: T) => void, operation?: Pick<Operation, "method" | "body">) => void;
export type ModelAuthorization = { request_id: string; plan_id: string; task_id: string; connection_id: string; connection_revision: string; model: string; status: "draft" | "authorized" | "running" | "succeeded" | "unknown" | "cancelled" | "expired" | "invalidated"; digest: string; created_at_unix_ms: string; expires_at_unix_ms: string; approved_at_unix_ms: string | null; preview: ModelReviewPreview | null; advice?: ModelAdvice | null };
export type ModelAuthorizationPage = { items: ModelAuthorization[]; next_cursor: string | null };

export type ModelAdvice = { protocol_version: string; input_digest: string } & Record<keyof EvidenceBody, { verdict: "missing" | "unverified" | "supported"; reason: string; citations: { field: keyof EvidenceBody; quote: string }[] }>;
