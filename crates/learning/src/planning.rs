//! 输入应来自服务端同一用户的完整快照，不是客户端可提交的可信评估。
use personal_ai_domain::UserId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

mod snapshot;
use snapshot::{Snapshot, canonical_id, hash};

pub const LEARNING_VERSION: &str = "self-assessed-learning-v1";
pub const MAX_SKILLS: usize = 100;
pub const MAX_PREREQUISITES: usize = 8;
pub const MAX_ASSESSMENTS: usize = 1000;
pub const MAX_GOALS: usize = 10;
pub const MAX_TASKS: usize = 5;
/// 产品中的自评目标值，不是经验证的客观能力分数。
pub const TARGET_SCORE: u8 = 70;
pub const ASSESSMENT_MINUTES: u16 = 5;
pub const PRACTICE_MINUTES: u16 = 25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LearningError {
    InvalidIdentity,
    OwnerMismatch,
    InvalidRequest,
    InvalidSkill,
    DuplicateSkill,
    InvalidPrerequisite,
    Cycle,
    InvalidAssessment,
    DuplicateAssessment,
    AmbiguousAssessment,
    InvalidGoal,
    TooLarge,
    Encoding,
}

/// 名称是私有输入；不实现 Debug，也不接受 HTTP 直接反序列化。
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct SkillNode {
    pub user_id: String,
    pub skill_id: String,
    pub revision: u64,
    pub name: String,
    pub enabled: bool,
    pub deleted: bool,
    pub prerequisite_ids: Vec<String>,
}
/// 只表示用户主动填写的自评；不得由阅读、计划生成或耗时自动推断。
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct SelfAssessment {
    pub user_id: String,
    pub assessment_id: String,
    pub skill_id: String,
    pub skill_revision: u64,
    pub score: u8,
    pub assessed_at_unix_ms: u64,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct LearningRequest {
    pub request_id: String,
    pub as_of_unix_ms: u64,
    pub budget_minutes: u16,
    pub goal_skill_ids: Vec<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub enum SkillState {
    Unavailable,
    Blocked,
    NeedsAssessment,
    NeedsPractice,
    Satisfied,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillEvaluation {
    pub skill_id: String,
    pub skill_revision: u64,
    pub name: String,
    pub state: SkillState,
    pub assessment_id: Option<String>,
    pub self_reported_score: Option<u8>,
    /// 全部未满足的传递先修技能，按规范 UUID 排序。
    pub blocking_skill_ids: Vec<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub enum TrainingKind {
    SelfAssessment,
    Practice,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrainingTask {
    pub task_id: String,
    pub skill_id: String,
    pub skill_revision: u64,
    pub kind: TrainingKind,
    pub title: String,
    pub instructions: String,
    pub target_minutes: u16,
    pub target_score: u8,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningPlan {
    pub version: String,
    pub user_id: String,
    pub request_id: String,
    pub as_of_unix_ms: u64,
    pub budget_minutes: u16,
    pub goal_skill_ids: Vec<String>,
    /// 包括未选中、停用和旧版本输入；不是执行授权或签名。
    pub input_digest: String,
    /// 仅包含目标及其全部先修技能，按稳定拓扑顺序排列。
    pub evaluations: Vec<SkillEvaluation>,
    pub tasks: Vec<TrainingTask>,
    pub unscheduled_ready_count: usize,
    pub remaining_minutes: u16,
}
impl LearningPlan {
    /// 绑定完整输出；后续仓储需保存原计划，重试时不能重新计算并替换。
    /// # Errors
    /// 序列化失败则拒绝生成摘要。
    pub fn digest(&self) -> Result<String, LearningError> {
        hash(self)
    }
}

/// 校验并规范化完整技能图，供仓储写入前检查使用。
/// # Errors
/// 拒绝跨用户、无效节点、重复/缺失先修引用、循环和超限图。
pub fn normalize_skill_graph(
    owner: &UserId,
    skills: &[SkillNode],
) -> Result<Vec<SkillNode>, LearningError> {
    let owner = canonical_id(owner.as_str())?;
    Ok(Snapshot::load(&owner, 0, skills, &[])?
        .skills
        .into_values()
        .collect())
}

/// 校验完整快照，生成显式目标的离线计划；不会更改自评或认为任务已完成。
/// # Errors
/// 拒绝跨用户、无效 UUID/版本/时间、超限、重复或矛盾自评、缺失引用、环和无效目标。
pub fn plan_learning(
    owner: &UserId,
    request: &LearningRequest,
    skills: &[SkillNode],
    assessments: &[SelfAssessment],
) -> Result<LearningPlan, LearningError> {
    let owner = canonical_id(owner.as_str())?;
    let request = normalize_request(request)?;
    let snapshot = Snapshot::load(&owner, request.as_of_unix_ms, skills, assessments)?;
    let mut relevant = BTreeSet::new();
    for goal in &request.goal_skill_ids {
        let node = snapshot
            .skills
            .get(goal)
            .ok_or(LearningError::InvalidGoal)?;
        if !node.enabled || node.deleted {
            return Err(LearningError::InvalidGoal);
        }
        relevant.insert(goal.clone());
        relevant.extend(snapshot.ancestors[goal].iter().cloned());
    }
    let input_digest = hash(&(
        LEARNING_VERSION,
        &owner,
        &request,
        &snapshot.skills,
        &snapshot.assessments,
    ))?;
    let evaluations = evaluate(&snapshot, &relevant);
    let (tasks, remaining_minutes, unscheduled_ready_count) =
        schedule(&owner, &request, &input_digest, &evaluations);
    Ok(LearningPlan {
        version: LEARNING_VERSION.into(),
        user_id: owner,
        request_id: request.request_id,
        as_of_unix_ms: request.as_of_unix_ms,
        budget_minutes: request.budget_minutes,
        goal_skill_ids: request.goal_skill_ids,
        input_digest,
        evaluations,
        tasks,
        unscheduled_ready_count,
        remaining_minutes,
    })
}
fn normalize_request(input: &LearningRequest) -> Result<LearningRequest, LearningError> {
    if input.goal_skill_ids.is_empty()
        || input.goal_skill_ids.len() > MAX_GOALS
        || !(5..=180).contains(&input.budget_minutes)
        || input.as_of_unix_ms > i64::MAX as u64
    {
        return Err(LearningError::InvalidRequest);
    }
    let mut goals = BTreeSet::new();
    for goal in &input.goal_skill_ids {
        if !goals.insert(canonical_id(goal)?) {
            return Err(LearningError::InvalidGoal);
        }
    }
    Ok(LearningRequest {
        request_id: canonical_id(&input.request_id)?,
        goal_skill_ids: goals.into_iter().collect(),
        ..input.clone()
    })
}
fn evaluate(snapshot: &Snapshot, relevant: &BTreeSet<String>) -> Vec<SkillEvaluation> {
    let mut states = BTreeMap::new();
    let mut result = Vec::new();
    for id in &snapshot.order {
        if !relevant.contains(id) {
            continue;
        }
        let node = &snapshot.skills[id];
        let latest = snapshot.latest.get(id).filter(|_| !node.deleted);
        let score = latest.map(|a| a.score);
        let blocking_skill_ids: Vec<_> = snapshot.ancestors[id]
            .iter()
            .filter(|ancestor| states.get(*ancestor) != Some(&SkillState::Satisfied))
            .cloned()
            .collect();
        let state = if !node.enabled || node.deleted {
            SkillState::Unavailable
        } else if !blocking_skill_ids.is_empty() {
            SkillState::Blocked
        } else {
            match score {
                None => SkillState::NeedsAssessment,
                Some(n) if n < TARGET_SCORE => SkillState::NeedsPractice,
                Some(_) => SkillState::Satisfied,
            }
        };
        states.insert(id.clone(), state);
        result.push(SkillEvaluation {
            skill_id: id.clone(),
            skill_revision: node.revision,
            name: if node.deleted {
                "已删除技能".into()
            } else {
                node.name.clone()
            },
            state,
            assessment_id: latest.map(|a| a.assessment_id.clone()),
            self_reported_score: score,
            blocking_skill_ids,
        });
    }
    result
}
fn schedule(
    owner: &str,
    request: &LearningRequest,
    digest: &str,
    evaluations: &[SkillEvaluation],
) -> (Vec<TrainingTask>, u16, usize) {
    let mut ready: Vec<_> = evaluations
        .iter()
        .filter(|e| {
            matches!(
                e.state,
                SkillState::NeedsAssessment | SkillState::NeedsPractice
            )
        })
        .collect();
    // None 排在 Some 之前，未知先评估；其后按自评分升序、规范 UUID 破同分。
    ready.sort_by(|a, b| {
        a.self_reported_score
            .cmp(&b.self_reported_score)
            .then(a.skill_id.cmp(&b.skill_id))
    });
    let mut remaining = request.budget_minutes;
    let mut tasks = Vec::new();
    let count = ready.len();
    for entry in ready {
        let (kind, minutes, title, instructions) = if entry.state == SkillState::NeedsAssessment {
            (
                TrainingKind::SelfAssessment,
                ASSESSMENT_MINUTES,
                "自评",
                "回顾自己能独立完成的操作，主动填写 0–100 分自评；不确定时可以暂不提交。",
            )
        } else {
            (
                TrainingKind::Practice,
                PRACTICE_MINUTES,
                "练习",
                "用自己的话解释一个关键概念，并完成一个小练习；记录结果后再主动更新自评。",
            )
        };
        if tasks.len() >= MAX_TASKS || minutes > remaining {
            continue;
        }
        let key = format!(
            "{LEARNING_VERSION}:{owner}:{}:{digest}:{}:{kind:?}",
            request.request_id, entry.skill_id
        );
        let task_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, key.as_bytes()).to_string();
        tasks.push(TrainingTask {
            task_id,
            skill_id: entry.skill_id.clone(),
            skill_revision: entry.skill_revision,
            kind,
            title: format!("{title}：{}", entry.name),
            instructions: instructions.into(),
            target_minutes: minutes,
            target_score: TARGET_SCORE,
        });
        remaining -= minutes;
    }
    let omitted = count - tasks.len();
    (tasks, remaining, omitted)
}

#[cfg(test)]
mod tests;
