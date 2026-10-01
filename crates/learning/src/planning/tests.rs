use super::*;
fn id(n: u128) -> String {
    Uuid::from_u128(n).to_string()
}
fn owner() -> UserId {
    UserId::new(id(1))
}
fn skill(n: u128, parents: &[u128]) -> SkillNode {
    SkillNode {
        user_id: id(1),
        skill_id: id(n),
        revision: 1,
        name: format!("技能 {n}"),
        enabled: true,
        deleted: false,
        prerequisite_ids: parents.iter().map(|p| id(*p)).collect(),
    }
}
fn assessment(n: u128, s: u128, score: u8, at: u64) -> SelfAssessment {
    SelfAssessment {
        user_id: id(1),
        assessment_id: id(n),
        skill_id: id(s),
        skill_revision: 1,
        score,
        assessed_at_unix_ms: at,
    }
}
fn request(goals: &[u128], budget: u16) -> LearningRequest {
    LearningRequest {
        request_id: id(2),
        as_of_unix_ms: 100,
        budget_minutes: budget,
        goal_skill_ids: goals.iter().map(|g| id(*g)).collect(),
    }
}
fn plan(
    goals: &[u128],
    budget: u16,
    skills: &[SkillNode],
    ratings: &[SelfAssessment],
) -> LearningPlan {
    plan_learning(&owner(), &request(goals, budget), skills, ratings).unwrap()
}
fn rejected(
    error: LearningError,
    req: &LearningRequest,
    skills: &[SkillNode],
    ratings: &[SelfAssessment],
) {
    assert_eq!(
        plan_learning(&owner(), req, skills, ratings).err(),
        Some(error)
    );
}

#[test]
fn unknown_is_not_zero_and_planning_never_completes_a_prerequisite() {
    let skills = [skill(10, &[]), skill(20, &[10])];
    let unknown = plan(&[20], 180, &skills, &[]);
    assert_eq!(unknown.tasks.len(), 1);
    assert_eq!(unknown.tasks[0].kind, TrainingKind::SelfAssessment);
    assert_eq!(unknown.tasks[0].target_minutes, 5);
    assert_eq!(unknown.evaluations[0].self_reported_score, None);
    assert_eq!(unknown.evaluations[1].state, SkillState::Blocked);
    assert_eq!(unknown.evaluations[1].blocking_skill_ids, vec![id(10)]);
    let low = plan(&[20], 180, &skills, &[assessment(100, 10, 0, 10)]);
    assert_eq!(low.tasks.len(), 1);
    assert_eq!(low.tasks[0].kind, TrainingKind::Practice);
    assert_eq!(low.tasks[0].target_minutes, 25);
    assert_eq!(low.evaluations[0].self_reported_score, Some(0));
    assert_eq!(low.evaluations[1].state, SkillState::Blocked);
}

#[test]
fn transitive_prerequisites_gate_even_a_high_scored_intermediate_skill() {
    let skills = [skill(10, &[]), skill(20, &[10]), skill(30, &[20])];
    let ratings = [assessment(100, 10, 69, 10), assessment(101, 20, 100, 10)];
    let saved = plan(&[30], 180, &skills, &ratings);
    assert_eq!(
        saved.evaluations[2].blocking_skill_ids,
        vec![id(10), id(20)]
    );
    assert_eq!(saved.tasks[0].skill_id, id(10));
    let ratings = [assessment(100, 10, 70, 10), assessment(101, 20, 100, 10)];
    let saved = plan(&[30], 180, &skills, &ratings);
    assert_eq!(saved.tasks.len(), 1);
    assert_eq!(saved.tasks[0].skill_id, id(30));
    assert_eq!(saved.evaluations[1].state, SkillState::Satisfied);
}

#[test]
fn satisfied_goals_do_not_schedule_extra_work_or_promote_other_skills() {
    let saved = plan(
        &[10],
        180,
        &[skill(10, &[]), skill(20, &[])],
        &[assessment(100, 10, 70, 10)],
    );
    assert!(saved.tasks.is_empty());
    assert_eq!(saved.evaluations.len(), 1);
    assert_eq!(saved.evaluations[0].state, SkillState::Satisfied);
    assert_eq!(saved.remaining_minutes, 180);
}

#[test]
fn latest_self_report_can_lower_a_score_and_stale_revisions_require_reassessment() {
    let ratings = [assessment(100, 10, 100, 1), assessment(101, 10, 20, 2)];
    let saved = plan(&[10], 30, &[skill(10, &[])], &ratings);
    assert_eq!(saved.evaluations[0].self_reported_score, Some(20));
    assert_eq!(saved.evaluations[0].assessment_id, Some(id(101)));
    let mut node = skill(10, &[]);
    node.revision = 2;
    let saved = plan(&[10], 30, &[node], &ratings);
    assert_eq!(saved.evaluations[0].self_reported_score, None);
    assert_eq!(saved.tasks[0].kind, TrainingKind::SelfAssessment);
}

#[test]
fn disabled_and_deleted_prerequisites_block_and_cannot_be_goals() {
    for deleted in [false, true] {
        let mut first = skill(10, &[]);
        first.enabled = false;
        first.deleted = deleted;
        let skills = [first, skill(20, &[10])];
        let saved = plan(&[20], 180, &skills, &[assessment(100, 10, 100, 1)]);
        assert!(saved.tasks.is_empty());
        assert_eq!(saved.evaluations[0].state, SkillState::Unavailable);
        assert_eq!(saved.evaluations[1].state, SkillState::Blocked);
        rejected(
            LearningError::InvalidGoal,
            &request(&[10], 25),
            &skills,
            &[],
        );
    }
}

#[test]
fn graphs_reject_missing_duplicate_and_self_edges_and_cycles_even_outside_goals() {
    let req = request(&[10], 25);
    rejected(
        LearningError::InvalidPrerequisite,
        &req,
        &[skill(10, &[99])],
        &[],
    );
    rejected(
        LearningError::InvalidPrerequisite,
        &req,
        &[skill(10, &[10])],
        &[],
    );
    rejected(
        LearningError::InvalidPrerequisite,
        &req,
        &[skill(10, &[20, 20]), skill(20, &[])],
        &[],
    );
    rejected(
        LearningError::DuplicateSkill,
        &req,
        &[skill(10, &[]), skill(10, &[])],
        &[],
    );
    rejected(
        LearningError::Cycle,
        &req,
        &[skill(10, &[]), skill(20, &[30]), skill(30, &[20])],
        &[],
    );
    rejected(
        LearningError::Cycle,
        &req,
        &[skill(10, &[20]), skill(20, &[30]), skill(30, &[10])],
        &[],
    );
}

#[test]
fn owner_checks_cover_unrelated_disabled_skills_and_stale_assessments() {
    let mut foreign = skill(20, &[]);
    foreign.user_id = id(99);
    foreign.enabled = false;
    rejected(
        LearningError::OwnerMismatch,
        &request(&[10], 25),
        &[skill(10, &[]), foreign],
        &[],
    );
    let mut node = skill(10, &[]);
    node.revision = 2;
    let mut foreign = assessment(100, 10, 100, 1);
    foreign.user_id = id(99);
    rejected(
        LearningError::OwnerMismatch,
        &request(&[10], 25),
        &[node],
        &[foreign],
    );
    assert_eq!(
        plan_learning(
            &UserId::new(id(99)),
            &request(&[10], 25),
            &[skill(10, &[])],
            &[]
        )
        .err(),
        Some(LearningError::OwnerMismatch)
    );
}

#[test]
fn invalid_assessments_are_not_clamped_or_ignored() {
    let req = request(&[10], 25);
    let skills = [skill(10, &[])];
    for a in [
        assessment(100, 10, 101, 1),
        assessment(100, 10, 255, 1),
        assessment(100, 99, 50, 1),
        assessment(100, 10, 50, 101),
    ] {
        rejected(LearningError::InvalidAssessment, &req, &skills, &[a]);
    }
    for revision in [0, 2, u64::MAX] {
        let mut a = assessment(100, 10, 50, 1);
        a.skill_revision = revision;
        rejected(LearningError::InvalidAssessment, &req, &skills, &[a]);
    }
    let a = assessment(100, 10, 50, 1);
    rejected(
        LearningError::DuplicateAssessment,
        &req,
        &skills,
        &[a.clone(), a],
    );
    rejected(
        LearningError::AmbiguousAssessment,
        &req,
        &skills,
        &[assessment(100, 10, 10, 1), assessment(101, 10, 90, 1)],
    );
    let saved = plan(
        &[10],
        25,
        &skills,
        &[assessment(100, 10, 50, 1), assessment(101, 10, 50, 1)],
    );
    assert_eq!(saved.evaluations[0].assessment_id, Some(id(101)));
}

#[test]
fn stable_normalization_covers_input_goal_and_edge_order_and_uuid_spelling() {
    let skills = vec![skill(10, &[]), skill(20, &[]), skill(30, &[10, 20])];
    let ratings = vec![assessment(100, 10, 80, 1), assessment(101, 20, 90, 2)];
    let original = plan(&[30, 10], 30, &skills, &ratings);
    let mut shuffled = skills;
    shuffled.reverse();
    shuffled[0].prerequisite_ids.reverse();
    for node in &mut shuffled {
        node.user_id = node.user_id.to_uppercase();
        node.skill_id = Uuid::parse_str(&node.skill_id)
            .unwrap()
            .simple()
            .to_string();
        node.name = format!(" {} ", node.name);
    }
    let mut reversed = ratings;
    reversed.reverse();
    let reordered = plan(&[10, 30], 30, &shuffled, &reversed);
    assert!(original == reordered);
    assert_eq!(original.digest().unwrap(), reordered.digest().unwrap());
}

#[test]
fn limits_are_checked_before_cloning_and_budgets_are_never_silently_clamped() {
    let req = request(&[10], 25);
    rejected(
        LearningError::TooLarge,
        &req,
        &vec![skill(10, &[]); MAX_SKILLS + 1],
        &[],
    );
    rejected(
        LearningError::TooLarge,
        &req,
        &[skill(10, &[])],
        &vec![assessment(100, 10, 1, 1); MAX_ASSESSMENTS + 1],
    );
    rejected(
        LearningError::TooLarge,
        &req,
        &[skill(10, &[20, 21, 22, 23, 24, 25, 26, 27, 28])],
        &[],
    );
    for budget in [0, 4, 181, u16::MAX] {
        rejected(
            LearningError::InvalidRequest,
            &request(&[10], budget),
            &[skill(10, &[])],
            &[],
        );
    }
    rejected(
        LearningError::InvalidRequest,
        &request(&[], 25),
        &[skill(10, &[])],
        &[],
    );
    rejected(
        LearningError::InvalidRequest,
        &request(&[10; MAX_GOALS + 1], 25),
        &[skill(10, &[])],
        &[],
    );
    rejected(
        LearningError::InvalidGoal,
        &request(&[10, 10], 25),
        &[skill(10, &[])],
        &[],
    );
    rejected(
        LearningError::InvalidGoal,
        &request(&[99], 25),
        &[skill(10, &[])],
        &[],
    );
}

#[test]
fn ready_tasks_respect_time_and_count_with_unknown_before_lowest_score() {
    let skills: Vec<_> = (10..20).map(|n| skill(n, &[])).collect();
    let goals: Vec<_> = (10..20).collect();
    let ratings: Vec<_> = (11..20)
        .map(|n| assessment(n + 100, n, u8::try_from(20 - n).unwrap(), 1))
        .collect();
    let saved = plan(&goals, 30, &skills, &ratings);
    assert_eq!(
        saved
            .tasks
            .iter()
            .map(|t| t.skill_id.clone())
            .collect::<Vec<_>>(),
        vec![id(10), id(19)]
    );
    assert_eq!(saved.remaining_minutes, 0);
    assert_eq!(saved.unscheduled_ready_count, 8);
    let limited = plan(&goals, 180, &skills, &[]);
    assert_eq!(limited.tasks.len(), 5);
    assert_eq!(limited.unscheduled_ready_count, 5);
    assert_eq!(limited.remaining_minutes, 155);
    let no_fit = plan(&[10], 24, &[skill(10, &[])], &[assessment(100, 10, 1, 1)]);
    assert!(no_fit.tasks.is_empty());
    assert_eq!(no_fit.unscheduled_ready_count, 1);
    assert_eq!(no_fit.remaining_minutes, 24);
}

#[test]
fn bounded_diamond_and_maximum_depth_are_iterative_and_deduplicate_ancestors() {
    let skills = [
        skill(10, &[]),
        skill(20, &[10]),
        skill(30, &[10]),
        skill(40, &[20, 30]),
    ];
    let saved = plan(&[40], 180, &skills, &[]);
    assert_eq!(
        saved.evaluations[3].blocking_skill_ids,
        vec![id(10), id(20), id(30)]
    );
    assert_eq!(saved.tasks.len(), 1);
    let chain: Vec<_> = (10..110)
        .map(|n| skill(n, &if n == 10 { vec![] } else { vec![n - 1] }))
        .collect();
    let saved = plan(&[109], 180, &chain, &[]);
    assert_eq!(saved.evaluations.len(), 100);
    assert_eq!(saved.evaluations[99].blocking_skill_ids.len(), 99);
    assert_eq!(saved.tasks.len(), 1);
}

#[test]
fn private_text_and_identifiers_are_validated_before_planning() {
    for name in [
        String::new(),
        " ".into(),
        "x\nsecret".into(),
        "x".repeat(121),
    ] {
        let mut s = skill(10, &[]);
        s.name = name;
        rejected(LearningError::InvalidSkill, &request(&[10], 25), &[s], &[]);
    }
    for revision in [0, u64::MAX] {
        let mut s = skill(10, &[]);
        s.revision = revision;
        rejected(LearningError::InvalidSkill, &request(&[10], 25), &[s], &[]);
    }
    let mut req = request(&[10], 25);
    req.as_of_unix_ms = u64::MAX;
    rejected(LearningError::InvalidRequest, &req, &[skill(10, &[])], &[]);
    for invalid in ["bad".to_owned(), Uuid::nil().to_string()] {
        let mut req = request(&[10], 25);
        req.request_id = invalid.clone();
        rejected(LearningError::InvalidIdentity, &req, &[skill(10, &[])], &[]);
        let mut s = skill(10, &[]);
        s.skill_id = invalid;
        rejected(
            LearningError::InvalidIdentity,
            &request(&[10], 25),
            &[s],
            &[],
        );
    }
}

#[test]
fn digests_and_task_ids_bind_request_owner_budget_snapshot_and_outputs() {
    let skills = [skill(10, &[])];
    let req = request(&[10], 25);
    let saved = plan_learning(&owner(), &req, &skills, &[]).unwrap();
    assert_eq!(saved.input_digest.len(), 64);
    assert_eq!(saved.digest().unwrap().len(), 64);
    assert!(!Uuid::parse_str(&saved.tasks[0].task_id).unwrap().is_nil());
    for changed in [
        LearningRequest {
            request_id: id(3),
            ..req.clone()
        },
        LearningRequest {
            budget_minutes: 30,
            ..req.clone()
        },
        LearningRequest {
            as_of_unix_ms: 101,
            ..req.clone()
        },
    ] {
        let other = plan_learning(&owner(), &changed, &skills, &[]).unwrap();
        assert_ne!(saved.input_digest, other.input_digest);
        assert_ne!(saved.tasks[0].task_id, other.tasks[0].task_id);
    }
    let mut node = skill(10, &[]);
    node.user_id = id(9);
    let other = plan_learning(&UserId::new(id(9)), &req, &[node], &[]).unwrap();
    assert_ne!(saved.tasks[0].task_id, other.tasks[0].task_id);
    let unrelated = plan(&[10], 25, &[skill(10, &[]), skill(20, &[])], &[]);
    assert_ne!(saved.input_digest, unrelated.input_digest);
    let mut changed = saved.clone();
    changed.tasks[0].target_minutes += 1;
    assert_ne!(saved.digest().unwrap(), changed.digest().unwrap());
}

#[test]
fn deleted_dependencies_explain_blockers_without_returning_old_names_or_ratings() {
    let mut deleted = skill(10, &[]);
    deleted.deleted = true;
    deleted.name = "私人旧标题".into();
    let saved = plan(
        &[20],
        30,
        &[deleted, skill(20, &[10])],
        &[assessment(100, 10, 99, 1)],
    );
    assert_eq!(saved.evaluations[0].name, "已删除技能");
    assert_eq!(saved.evaluations[0].assessment_id, None);
    assert_eq!(saved.evaluations[0].self_reported_score, None);
    assert!(
        !serde_json::to_string(&saved)
            .unwrap()
            .contains("私人旧标题")
    );
    assert_eq!(saved.evaluations[1].blocking_skill_ids, vec![id(10)]);
}

#[test]
fn maximum_assessment_snapshot_and_exact_time_budgets_are_supported() {
    let ratings: Vec<_> = (1_u64..=1000)
        .map(|n| assessment(u128::from(n) + 100, 10, 20, n))
        .collect();
    let mut req = request(&[10], 25);
    req.as_of_unix_ms = 1000;
    let saved = plan_learning(&owner(), &req, &[skill(10, &[])], &ratings).unwrap();
    assert_eq!(saved.evaluations[0].assessment_id, Some(id(1100)));
    assert_eq!(saved.tasks.len(), 1);
    assert_eq!(saved.remaining_minutes, 0);
    let saved = plan(&[10], 5, &[skill(10, &[])], &[]);
    assert_eq!(saved.tasks.len(), 1);
    assert_eq!(saved.remaining_minutes, 0);
}

#[test]
fn irrelevant_and_stale_snapshot_changes_are_bound_without_becoming_task_evidence() {
    let mut current = skill(10, &[]);
    current.revision = 2;
    let mut unused = skill(20, &[]);
    unused.enabled = false;
    let skills = [current, unused];
    let first = plan(&[10], 25, &skills, &[assessment(100, 10, 20, 1)]);
    let changed = plan(&[10], 25, &skills, &[assessment(100, 10, 90, 1)]);
    assert_ne!(first.input_digest, changed.input_digest);
    assert_eq!(changed.tasks[0].kind, TrainingKind::SelfAssessment);
    assert_eq!(changed.evaluations[0].self_reported_score, None);
    let mut skills = skills;
    skills[1].name = "停用节点变更".into();
    let renamed = plan(&[10], 25, &skills, &[assessment(100, 10, 20, 1)]);
    assert_ne!(first.input_digest, renamed.input_digest);
    assert_eq!(renamed.evaluations.len(), 1);
}
