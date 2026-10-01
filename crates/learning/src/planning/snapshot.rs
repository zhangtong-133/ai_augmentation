use super::{
    BTreeMap, BTreeSet, Digest, LearningError, MAX_ASSESSMENTS, MAX_PREREQUISITES, MAX_SKILLS,
    SelfAssessment, Serialize, Sha256, SkillNode, Uuid,
};

pub(super) struct Snapshot {
    pub skills: BTreeMap<String, SkillNode>,
    pub assessments: BTreeMap<String, SelfAssessment>,
    pub latest: BTreeMap<String, SelfAssessment>,
    pub order: Vec<String>,
    pub ancestors: BTreeMap<String, BTreeSet<String>>,
}
pub(super) fn canonical_id(value: &str) -> Result<String, LearningError> {
    let id = Uuid::parse_str(value).map_err(|_| LearningError::InvalidIdentity)?;
    if id.is_nil() {
        return Err(LearningError::InvalidIdentity);
    }
    Ok(id.to_string())
}
pub(super) fn hash(value: &impl Serialize) -> Result<String, LearningError> {
    let bytes = serde_json::to_vec(value).map_err(|_| LearningError::Encoding)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn version(value: u64) -> bool {
    (1..=i64::MAX as u64).contains(&value)
}
fn owned(value: &str, owner: &str) -> Result<(), LearningError> {
    if canonical_id(value)? != owner {
        return Err(LearningError::OwnerMismatch);
    }
    Ok(())
}
impl Snapshot {
    pub fn load(
        owner: &str,
        as_of: u64,
        input: &[SkillNode],
        ratings: &[SelfAssessment],
    ) -> Result<Self, LearningError> {
        if input.len() > MAX_SKILLS || ratings.len() > MAX_ASSESSMENTS {
            return Err(LearningError::TooLarge);
        }
        let skills = normalize_skills(owner, input)?;
        let order = topological(&skills)?;
        let ancestors = ancestors(&skills, &order);
        let assessments = normalize_assessments(owner, as_of, &skills, ratings)?;
        let mut latest: BTreeMap<String, SelfAssessment> = BTreeMap::new();
        for assessment in assessments.values() {
            if assessment.skill_revision != skills[&assessment.skill_id].revision {
                continue;
            }
            let replace = latest.get(&assessment.skill_id).is_none_or(|previous| {
                (assessment.assessed_at_unix_ms, &assessment.assessment_id)
                    > (previous.assessed_at_unix_ms, &previous.assessment_id)
            });
            if replace {
                latest.insert(assessment.skill_id.clone(), assessment.clone());
            }
        }
        Ok(Self {
            skills,
            assessments,
            latest,
            order,
            ancestors,
        })
    }
}
fn normalize_skills(
    owner: &str,
    input: &[SkillNode],
) -> Result<BTreeMap<String, SkillNode>, LearningError> {
    let mut skills = BTreeMap::new();
    for node in input {
        owned(&node.user_id, owner)?;
        if !version(node.revision)
            || node.name.len() > 480
            || node.name.chars().count() > 120
            || node.name.trim().is_empty()
            || node.name.chars().any(char::is_control)
        {
            return Err(LearningError::InvalidSkill);
        }
        if node.prerequisite_ids.len() > MAX_PREREQUISITES {
            return Err(LearningError::TooLarge);
        }
        let skill_id = canonical_id(&node.skill_id)?;
        let mut parents = BTreeSet::new();
        for parent in &node.prerequisite_ids {
            let parent = canonical_id(parent)?;
            if parent == skill_id || !parents.insert(parent) {
                return Err(LearningError::InvalidPrerequisite);
            }
        }
        let node = SkillNode {
            user_id: owner.into(),
            skill_id: skill_id.clone(),
            name: node.name.trim().into(),
            prerequisite_ids: parents.into_iter().collect(),
            ..node.clone()
        };
        if skills.insert(skill_id, node).is_some() {
            return Err(LearningError::DuplicateSkill);
        }
    }
    Ok(skills)
}
fn topological(skills: &BTreeMap<String, SkillNode>) -> Result<Vec<String>, LearningError> {
    let mut degree = BTreeMap::new();
    let mut ready = BTreeSet::new();
    for (id, node) in skills {
        if node
            .prerequisite_ids
            .iter()
            .any(|p| !skills.contains_key(p))
        {
            return Err(LearningError::InvalidPrerequisite);
        }
        degree.insert(id.clone(), node.prerequisite_ids.len());
        if node.prerequisite_ids.is_empty() {
            ready.insert(id.clone());
        }
    }
    let mut order = Vec::new();
    while let Some(id) = ready.pop_first() {
        order.push(id.clone());
        for (child, node) in skills {
            if node.prerequisite_ids.contains(&id) {
                let count = degree.get_mut(child).expect("all skills have a degree");
                *count -= 1;
                if *count == 0 {
                    ready.insert(child.clone());
                }
            }
        }
    }
    if order.len() != skills.len() {
        return Err(LearningError::Cycle);
    }
    Ok(order)
}
fn ancestors(
    skills: &BTreeMap<String, SkillNode>,
    order: &[String],
) -> BTreeMap<String, BTreeSet<String>> {
    let mut result: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for id in order {
        let mut all = BTreeSet::new();
        for parent in &skills[id].prerequisite_ids {
            all.insert(parent.clone());
            all.extend(result[parent].iter().cloned());
        }
        result.insert(id.clone(), all);
    }
    result
}
fn normalize_assessments(
    owner: &str,
    as_of: u64,
    skills: &BTreeMap<String, SkillNode>,
    input: &[SelfAssessment],
) -> Result<BTreeMap<String, SelfAssessment>, LearningError> {
    let mut assessments = BTreeMap::new();
    let mut instants = BTreeMap::new();
    for rating in input {
        owned(&rating.user_id, owner)?;
        let skill_id = canonical_id(&rating.skill_id)?;
        let node = skills
            .get(&skill_id)
            .ok_or(LearningError::InvalidAssessment)?;
        if !version(rating.skill_revision)
            || rating.skill_revision > node.revision
            || rating.score > 100
            || rating.assessed_at_unix_ms > as_of
        {
            return Err(LearningError::InvalidAssessment);
        }
        let assessment_id = canonical_id(&rating.assessment_id)?;
        let instant = (
            skill_id.clone(),
            rating.skill_revision,
            rating.assessed_at_unix_ms,
        );
        if instants
            .insert(instant, rating.score)
            .is_some_and(|previous| previous != rating.score)
        {
            return Err(LearningError::AmbiguousAssessment);
        }
        let rating = SelfAssessment {
            user_id: owner.into(),
            assessment_id: assessment_id.clone(),
            skill_id,
            ..rating.clone()
        };
        if assessments.insert(assessment_id, rating).is_some() {
            return Err(LearningError::DuplicateAssessment);
        }
    }
    Ok(assessments)
}
