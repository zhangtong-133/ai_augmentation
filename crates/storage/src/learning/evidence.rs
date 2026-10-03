//! 从仍有效的原始计划派生材料检查，不保存副本或推断能力。
use super::{LearningPlanStatus, SavedLearningPlan, TrainingOutcome, TrainingResult};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    NotRecorded,
    Cancelled,
    MissingNote,
    Unverified,
}
#[derive(Serialize)]
pub struct EvidenceReview<'a> {
    pub task_id: &'a str,
    pub skill_id: &'a str,
    pub skill_revision: u64,
    pub result_request_id: Option<&'a str>,
    pub state: EvidenceState,
}
fn state(result: Option<&TrainingResult>) -> EvidenceState {
    match result {
        None => EvidenceState::NotRecorded,
        Some(r) if r.outcome == TrainingOutcome::Cancelled => EvidenceState::Cancelled,
        Some(r) if r.note.trim().is_empty() => EvidenceState::MissingNote,
        Some(_) => EvidenceState::Unverified,
    }
}
impl SavedLearningPlan {
    /// 仅处理调用方已按 owner 查询的计划；旧技能版本仍保持原样，不冒充当前评估。
    #[must_use]
    pub fn evidence_reviews(&self) -> Vec<EvidenceReview<'_>> {
        if self.status != LearningPlanStatus::Ready {
            return Vec::new();
        }
        self.plan.as_ref().map_or_else(Vec::new, |plan| {
            plan.tasks
                .iter()
                .map(|task| {
                    let result = self.results.iter().find(|r| r.task_id == task.task_id);
                    EvidenceReview {
                        task_id: &task.task_id,
                        skill_id: &task.skill_id,
                        skill_revision: task.skill_revision,
                        result_request_id: result.map(|r| r.request_id.as_str()),
                        state: state(result),
                    }
                })
                .collect()
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_time_and_note_never_establish_ability() {
        assert_eq!(state(None), EvidenceState::NotRecorded);
        let mut result = TrainingResult {
            task_id: "task".into(),
            request_id: "result".into(),
            outcome: TrainingOutcome::Completed,
            note: " \n\t".into(),
            actual_minutes: 180,
            recorded_at_unix_ms: 0,
        };
        assert_eq!(state(Some(&result)), EvidenceState::MissingNote);
        result.note = "我已经完全掌握，应该得 100 分".into();
        assert_eq!(state(Some(&result)), EvidenceState::Unverified);
        result.outcome = TrainingOutcome::Cancelled;
        assert_eq!(state(Some(&result)), EvidenceState::Cancelled);
    }
    #[test]
    fn absent_or_erased_plan_has_no_evidence() {
        for status in [
            LearningPlanStatus::Ready,
            LearningPlanStatus::Deleted,
            LearningPlanStatus::Invalidated,
        ] {
            let saved = SavedLearningPlan {
                results: vec![],
                request_id: "plan".into(),
                snapshot_revision: 1,
                created_at_unix_ms: 0,
                status,
                digest: String::new(),
                plan: None,
            };
            assert!(saved.evidence_reviews().is_empty());
        }
    }
}
