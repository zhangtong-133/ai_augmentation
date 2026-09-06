#![forbid(unsafe_code)]

use personal_ai_domain::SkillId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Skill {
    pub id: SkillId,
    pub name: String,
    pub proficiency: u8,
    pub prerequisite_ids: Vec<SkillId>,
}

impl Skill {
    #[must_use]
    pub fn normalized_proficiency(&self) -> u8 {
        self.proficiency.min(100)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrainingTask {
    pub skill_id: SkillId,
    pub title: String,
    pub target_minutes: u16,
}

#[must_use]
pub fn create_practice_task(skill: &Skill, target_minutes: u16) -> TrainingTask {
    TrainingTask {
        skill_id: skill.id.clone(),
        title: format!("Practice {}", skill.name),
        target_minutes: target_minutes.max(5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn practice_task_has_a_minimum_duration() {
        let skill = Skill {
            id: SkillId::new("rust"),
            name: "Rust".into(),
            proficiency: 20,
            prerequisite_ids: Vec::new(),
        };

        assert_eq!(create_practice_task(&skill, 1).target_minutes, 5);
    }
}
