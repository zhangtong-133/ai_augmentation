//! 当前用户的技能、自评和不可变训练计划；无执行、模型或通知副作用。
use crate::{BoxFuture, StorageResult};
use personal_ai_domain::UserId;
use personal_ai_learning::planning::{LearningPlan, SelfAssessment, SkillNode};
use serde::Serialize;

#[derive(Clone, PartialEq, Eq)]
pub struct SkillInput {
    pub name: String,
    pub enabled: bool,
    pub prerequisite_ids: Vec<String>,
}
#[derive(Clone, PartialEq, Eq)]
pub struct AssessmentInput {
    pub expected_revision: u64,
    pub skill_id: String,
    pub skill_revision: u64,
    pub score: u8,
}
#[derive(Clone, PartialEq, Eq)]
pub struct LearningPlanInput {
    pub expected_revision: u64,
    pub budget_minutes: u16,
    pub goal_skill_ids: Vec<String>,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct LearningSnapshot {
    pub revision: u64,
    pub skills: Vec<SkillNode>,
    pub assessments: Vec<SelfAssessment>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningPlanStatus {
    Ready,
    Invalidated,
    Deleted,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct SavedLearningPlan {
    pub results: Vec<TrainingResult>,
    pub request_id: String,
    pub snapshot_revision: u64,
    pub created_at_unix_ms: u64,
    pub status: LearningPlanStatus,
    pub digest: String,
    pub plan: Option<LearningPlan>,
}
#[derive(Clone, Serialize)]
pub struct LearningPlanSummary {
    pub request_id: String,
    pub created_at_unix_ms: u64,
    pub status: LearningPlanStatus,
}
#[derive(Clone, Serialize)]
pub struct LearningPlanPage {
    pub items: Vec<LearningPlanSummary>,
    pub next_cursor: Option<String>,
}
pub trait LearningStore: Send + Sync {
    fn record_training_result(
        &self,
        owner: &UserId,
        plan: &str,
        task: &str,
        input: &TrainingResultInput,
    ) -> BoxFuture<'_, StorageResult<SavedLearningPlan>>;

    fn learning_snapshot(&self, owner: &UserId) -> BoxFuture<'_, StorageResult<LearningSnapshot>>;
    /// `expected_revision=0` 创建技能，否则比较技能版本再修改。
    fn save_skill(
        &self,
        owner: &UserId,
        skill: &str,
        expected_revision: u64,
        input: &SkillInput,
    ) -> BoxFuture<'_, StorageResult<SkillNode>>;
    fn delete_skill(
        &self,
        owner: &UserId,
        skill: &str,
        expected_revision: u64,
    ) -> BoxFuture<'_, StorageResult<()>>;
    fn record_assessment(
        &self,
        owner: &UserId,
        request: &str,
        input: &AssessmentInput,
    ) -> BoxFuture<'_, StorageResult<SelfAssessment>>;
    fn create_learning_plan(
        &self,
        owner: &UserId,
        request: &str,
        input: &LearningPlanInput,
    ) -> BoxFuture<'_, StorageResult<SavedLearningPlan>>;
    fn get_learning_plan(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<SavedLearningPlan>>;
    fn list_learning_plans(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<LearningPlanPage>>;
    fn delete_learning_plan(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<()>>;
}

/// 用户显式记录的终结结果，不构成能力评估。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrainingOutcome {
    Completed,
    Cancelled,
}
#[derive(Clone, PartialEq, Eq)]
pub struct TrainingResultInput {
    pub request_id: String,
    pub outcome: TrainingOutcome,
    pub note: String,
    pub actual_minutes: u16,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct TrainingResult {
    pub task_id: String,
    pub request_id: String,
    pub outcome: TrainingOutcome,
    pub note: String,
    pub actual_minutes: u16,
    pub recorded_at_unix_ms: u64,
}
