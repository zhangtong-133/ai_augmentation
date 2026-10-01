use super::*;
use personal_ai_learning::planning::{SkillNode, SkillState, TrainingKind};
use personal_ai_storage::learning::{
    AssessmentInput, LearningPlanInput, LearningPlanStatus, LearningStore, SkillInput,
};
struct Fixture {
    store: PostgresStore,
    pool: sqlx::PgPool,
    owner: UserId,
    other: UserId,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let store = PostgresStore::connect(&url).await.unwrap();
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let owner = UserId::new(key());
        let other = UserId::new(key());
        for id in [&owner, &other] {
            store
                .save_user(&User {
                    id: id.clone(),
                    email: format!("{}@learning.example", id.as_str()),
                    display_name: "学习测试".into(),
                })
                .await
                .unwrap();
        }
        Self {
            store,
            pool,
            owner,
            other,
        }
    }
    fn owner(&self) -> Uuid {
        Uuid::parse_str(self.owner.as_str()).unwrap()
    }
    async fn skill(&self, parents: &[String]) -> SkillNode {
        self.store
            .save_skill(&self.owner, &key(), 0, &skill_input("私有技能", parents))
            .await
            .unwrap()
    }
    async fn version(&self) -> u64 {
        self.store
            .learning_snapshot(&self.owner)
            .await
            .unwrap()
            .revision
    }
    async fn plan_input(&self, node: &SkillNode) -> LearningPlanInput {
        LearningPlanInput {
            expected_revision: self.version().await,
            budget_minutes: 30,
            goal_skill_ids: vec![node.skill_id.clone()],
        }
    }
    async fn cleanup(&self) {
        sqlx::query("DELETE FROM users WHERE id=$1 OR id=$2")
            .bind(self.owner())
            .bind(Uuid::parse_str(self.other.as_str()).unwrap())
            .execute(&self.pool)
            .await
            .unwrap();
    }
}
fn key() -> String {
    Uuid::new_v4().to_string()
}
fn skill_input(name: &str, parents: &[String]) -> SkillInput {
    SkillInput {
        name: name.into(),
        enabled: true,
        prerequisite_ids: parents.to_vec(),
    }
}
fn conflict<T>(value: Result<T, StorageError>) {
    assert!(
        value
            .err()
            .is_some_and(|error| matches!(error, StorageError::Conflict(_)))
    );
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_skills_validate_graph_and_serialize_cross_node_cycles_and_versions() {
    let f = Fixture::new().await;
    assert_eq!(f.version().await, 0);
    let a = f.skill(&[]).await;
    let b = f.skill(&[]).await;
    assert_eq!(f.version().await, 2);
    assert!(
        f.store
            .save_skill(&f.owner, &a.skill_id, 0, &skill_input(" 私有技能 ", &[]))
            .await
            .unwrap()
            == a
    );
    let (left, right) = tokio::join!(
        f.store.save_skill(
            &f.owner,
            &a.skill_id,
            1,
            &skill_input("甲", std::slice::from_ref(&b.skill_id))
        ),
        f.store.save_skill(
            &f.owner,
            &b.skill_id,
            1,
            &skill_input("乙", std::slice::from_ref(&a.skill_id))
        )
    );
    assert_ne!(left.is_ok(), right.is_ok());
    assert_eq!(f.version().await, 3);
    let changed = left.ok().or_else(|| right.ok()).unwrap();
    conflict(
        f.store
            .save_skill(&f.owner, &changed.skill_id, 1, &skill_input("旧版本", &[]))
            .await,
    );
    assert!(matches!(
        f.store
            .save_skill(&f.other, &changed.skill_id, 1, &skill_input("外部", &[]))
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store
            .save_skill(&f.owner, &key(), 0, &skill_input("坏引用", &[key()]))
            .await,
        Err(StorageError::InvalidData(_))
    ));
    assert!(
        f.store
            .learning_snapshot(&f.other)
            .await
            .unwrap()
            .skills
            .is_empty()
    );
    assert_eq!(f.version().await, 3);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_assessments_are_explicit_versioned_private_and_idempotent() {
    let f = Fixture::new().await;
    let node = f.skill(&[]).await;
    let request = key();
    let input = AssessmentInput {
        expected_revision: 1,
        skill_id: node.skill_id.clone(),
        skill_revision: 1,
        score: 80,
    };
    let (a, b) = tokio::join!(
        f.store.record_assessment(&f.owner, &request, &input),
        f.store.record_assessment(&f.owner, &request, &input)
    );
    let saved = a.unwrap();
    assert!(saved == b.unwrap());
    assert_eq!(f.version().await, 2);
    conflict(
        f.store
            .record_assessment(
                &f.owner,
                &request,
                &AssessmentInput {
                    score: 40,
                    ..input.clone()
                },
            )
            .await,
    );
    conflict(f.store.record_assessment(&f.owner, &key(), &input).await);
    assert!(matches!(
        f.store
            .record_assessment(
                &f.other,
                &key(),
                &AssessmentInput {
                    expected_revision: 0,
                    ..input.clone()
                }
            )
            .await,
        Err(StorageError::NotFound)
    ));
    let changed = f
        .store
        .save_skill(&f.owner, &node.skill_id, 1, &skill_input("新含义", &[]))
        .await
        .unwrap();
    assert_eq!(changed.revision, 2);
    assert_eq!(f.version().await, 3);
    assert!(
        saved
            == f.store
                .record_assessment(&f.owner, &request, &input)
                .await
                .unwrap()
    );
    conflict(
        f.store
            .record_assessment(
                &f.owner,
                &key(),
                &AssessmentInput {
                    expected_revision: 3,
                    ..input.clone()
                },
            )
            .await,
    );
    let brief = f
        .store
        .create_learning_plan(&f.owner, &key(), &f.plan_input(&changed).await)
        .await
        .unwrap();
    assert_eq!(
        brief.plan.unwrap().tasks[0].kind,
        TrainingKind::SelfAssessment
    );
    assert_eq!(
        f.store
            .learning_snapshot(&f.owner)
            .await
            .unwrap()
            .assessments[0]
            .score,
        80
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_plan_and_tasks_are_frozen_private_and_request_bound() {
    let f = Fixture::new().await;
    let node = f.skill(&[]).await;
    let request = key();
    let input = f.plan_input(&node).await;
    let (a, b) = tokio::join!(
        f.store.create_learning_plan(&f.owner, &request, &input),
        f.store.create_learning_plan(&f.owner, &request, &input)
    );
    let saved = a.unwrap();
    assert!(saved == b.unwrap());
    assert_eq!(f.version().await, 1);
    assert_eq!(saved.plan.as_ref().unwrap().tasks.len(), 1);
    f.store
        .save_skill(&f.owner, &node.skill_id, 1, &skill_input("改名", &[]))
        .await
        .unwrap();
    assert!(
        saved
            == f.store
                .create_learning_plan(&f.owner, &request, &input)
                .await
                .unwrap()
    );
    conflict(
        f.store
            .create_learning_plan(
                &f.owner,
                &request,
                &LearningPlanInput {
                    budget_minutes: 60,
                    ..input.clone()
                },
            )
            .await,
    );
    conflict(f.store.create_learning_plan(&f.owner, &key(), &input).await);
    assert!(matches!(
        f.store.get_learning_plan(&f.other, &request).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store.delete_learning_plan(&f.other, &request).await,
        Err(StorageError::NotFound)
    ));
    assert!(
        f.store
            .list_learning_plans(&f.other, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert!(
        saved
            == reopened
                .get_learning_plan(&f.owner, &request)
                .await
                .unwrap()
    );
    sqlx::query(
        "UPDATE learning_tasks SET task=jsonb_set(task,'{title}','\"tampered\"') WHERE user_id=$1",
    )
    .bind(f.owner())
    .execute(&f.pool)
    .await
    .unwrap();
    conflict(f.store.get_learning_plan(&f.owner, &request).await);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn learning_source_delete_scrubs_assessments_plans_tasks_and_prevents_resurrection() {
    let f = Fixture::new().await;
    let a = f.skill(&[]).await;
    let b = f.skill(std::slice::from_ref(&a.skill_id)).await;
    let unrelated = f.skill(&[]).await;
    let rating = key();
    let assess = AssessmentInput {
        expected_revision: 3,
        skill_id: a.skill_id.clone(),
        skill_revision: 1,
        score: 80,
    };
    f.store
        .record_assessment(&f.owner, &rating, &assess)
        .await
        .unwrap();
    let request = key();
    let plan = f.plan_input(&b).await;
    f.store
        .create_learning_plan(&f.owner, &request, &plan)
        .await
        .unwrap();
    // 即使不是目标或其先修，完整快照中的来源删除也使原计划失效。
    f.store
        .delete_skill(&f.owner, &unrelated.skill_id, 1)
        .await
        .unwrap();
    let invalidated = f.store.get_learning_plan(&f.owner, &request).await.unwrap();
    assert_eq!(invalidated.status, LearningPlanStatus::Invalidated);
    assert!(invalidated.plan.is_none());
    assert!(
        invalidated
            == f.store
                .create_learning_plan(&f.owner, &request, &plan)
                .await
                .unwrap()
    );
    f.store
        .delete_skill(&f.owner, &a.skill_id, 1)
        .await
        .unwrap();
    f.store
        .delete_skill(&f.owner, &a.skill_id, 1)
        .await
        .unwrap();
    assert_eq!(f.version().await, 6);
    assert!(
        f.store
            .learning_snapshot(&f.owner)
            .await
            .unwrap()
            .assessments
            .is_empty()
    );
    conflict(f.store.record_assessment(&f.owner, &rating, &assess).await);
    conflict(
        f.store
            .save_skill(&f.owner, &a.skill_id, 0, &skill_input("复活", &[]))
            .await,
    );
    let new = f
        .store
        .create_learning_plan(&f.owner, &key(), &f.plan_input(&b).await)
        .await
        .unwrap();
    assert!(new.plan.as_ref().unwrap().tasks.is_empty());
    assert!(
        new.plan
            .unwrap()
            .evaluations
            .iter()
            .any(|e| e.skill_id == b.skill_id && e.state == SkillState::Blocked)
    );
    let private: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM learning_tasks WHERE user_id=$1 AND task IS NOT NULL",
    )
    .bind(f.owner())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(private, 0);
    f.store
        .delete_learning_plan(&f.owner, &request)
        .await
        .unwrap();
    f.store
        .delete_learning_plan(&f.owner, &request)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .create_learning_plan(&f.owner, &request, &plan)
            .await
            .unwrap()
            .status,
        LearningPlanStatus::Deleted
    );
    f.cleanup().await;
    for table in [
        "learning_state",
        "learning_skills",
        "learning_edges",
        "learning_assessments",
        "learning_plans",
        "learning_plan_sources",
        "learning_tasks",
    ] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE user_id=$1"))
            .bind(f.owner())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_create_delete_race_never_retains_deleted_skill_content() {
    let f = Fixture::new().await;
    let node = f.skill(&[]).await;
    let request = key();
    let input = f.plan_input(&node).await;
    let (created, deleted) = tokio::join!(
        f.store.create_learning_plan(&f.owner, &request, &input),
        f.store.delete_skill(&f.owner, &node.skill_id, 1)
    );
    deleted.unwrap();
    match created {
        Ok(_) => assert!(
            f.store
                .get_learning_plan(&f.owner, &request)
                .await
                .unwrap()
                .plan
                .is_none()
        ),
        Err(e) => assert!(matches!(e, StorageError::Conflict(_))),
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_task_write_failure_rolls_back_plan_sources_and_preserves_retry() {
    let f = Fixture::new().await;
    let node = f.skill(&[]).await;
    let request = Uuid::new_v4();
    let input = f.plan_input(&node).await;
    let constraint = format!("learning_fault_{}", request.simple());
    sqlx::query(&format!("ALTER TABLE learning_tasks ADD CONSTRAINT {constraint} CHECK(request_id<>'{request}') NOT VALID")).execute(&f.pool).await.unwrap();
    assert!(
        f.store
            .create_learning_plan(&f.owner, &request.to_string(), &input)
            .await
            .is_err()
    );
    assert!(matches!(
        f.store
            .get_learning_plan(&f.owner, &request.to_string())
            .await,
        Err(StorageError::NotFound)
    ));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM learning_plan_sources WHERE user_id=$1")
        .bind(f.owner())
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(f.version().await, 1);
    sqlx::query(&format!(
        "ALTER TABLE learning_tasks DROP CONSTRAINT {constraint}"
    ))
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        f.store
            .create_learning_plan(&f.owner, &request.to_string(), &input)
            .await
            .unwrap()
            .plan
            .unwrap()
            .tasks
            .len(),
        1
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_hard_delete_invalidates_and_account_cascade_clears_ready_plans() {
    let f = Fixture::new().await;
    let node = f.skill(&[]).await;
    let request = key();
    f.store
        .create_learning_plan(&f.owner, &request, &f.plan_input(&node).await)
        .await
        .unwrap();
    sqlx::query("DELETE FROM learning_skills WHERE user_id=$1 AND id=$2")
        .bind(f.owner())
        .bind(Uuid::parse_str(&node.skill_id).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .get_learning_plan(&f.owner, &request)
            .await
            .unwrap()
            .status,
        LearningPlanStatus::Invalidated
    );
    let a = f.skill(&[]).await;
    let b = f.skill(std::slice::from_ref(&a.skill_id)).await;
    f.store
        .create_learning_plan(&f.owner, &key(), &f.plan_input(&b).await)
        .await
        .unwrap();
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_daily_quota_counts_deleted_plans_but_preserves_original_replay() {
    let f = Fixture::new().await;
    let node = f.skill(&[]).await;
    let input = f.plan_input(&node).await;
    let first = key();
    f.store
        .create_learning_plan(&f.owner, &first, &input)
        .await
        .unwrap();
    for _ in 1..10 {
        let id = key();
        f.store
            .create_learning_plan(&f.owner, &id, &input)
            .await
            .unwrap();
        f.store.delete_learning_plan(&f.owner, &id).await.unwrap();
    }
    conflict(f.store.create_learning_plan(&f.owner, &key(), &input).await);
    assert_eq!(
        f.store
            .create_learning_plan(&f.owner, &first, &input)
            .await
            .unwrap()
            .status,
        LearningPlanStatus::Ready
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_history_pages_are_bounded_private_and_lifetime_quota_is_persistent() {
    let f = Fixture::new().await;
    let node = f.skill(&[]).await;
    sqlx::query("INSERT INTO learning_plans(user_id,request_id,snapshot_revision,request_digest,digest,created_ms,status) SELECT $1,gen_random_uuid(),1,repeat('a',64),repeat('a',64),0,'deleted' FROM generate_series(1,1000)").bind(f.owner()).execute(&f.pool).await.unwrap();
    conflict(
        f.store
            .create_learning_plan(&f.owner, &key(), &f.plan_input(&node).await)
            .await,
    );
    let mut cursor = None;
    let mut ids = std::collections::BTreeSet::new();
    loop {
        let page = f
            .store
            .list_learning_plans(&f.owner, cursor.as_deref())
            .await
            .unwrap();
        assert!(page.items.len() <= 20);
        for item in page.items {
            assert!(ids.insert(item.request_id));
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), 1000);
    assert!(
        f.store
            .list_learning_plans(&f.other, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_revision_failure_rolls_back_skill_assessment_and_delete_side_effects() {
    let f = Fixture::new().await;
    let node = f.skill(&[]).await;
    f.store
        .record_assessment(
            &f.owner,
            &key(),
            &AssessmentInput {
                expected_revision: 1,
                skill_id: node.skill_id.clone(),
                skill_revision: 1,
                score: 20,
            },
        )
        .await
        .unwrap();
    let request = key();
    let plan = f
        .store
        .create_learning_plan(&f.owner, &request, &f.plan_input(&node).await)
        .await
        .unwrap();
    sqlx::query("UPDATE learning_state SET revision=9223372036854775807 WHERE user_id=$1")
        .bind(f.owner())
        .execute(&f.pool)
        .await
        .unwrap();
    conflict(
        f.store
            .save_skill(&f.owner, &node.skill_id, 1, &skill_input("不能写入", &[]))
            .await,
    );
    conflict(
        f.store
            .record_assessment(
                &f.owner,
                &key(),
                &AssessmentInput {
                    expected_revision: i64::MAX as u64,
                    skill_id: node.skill_id.clone(),
                    skill_revision: 1,
                    score: 80,
                },
            )
            .await,
    );
    conflict(f.store.delete_skill(&f.owner, &node.skill_id, 1).await);
    let snapshot = f.store.learning_snapshot(&f.owner).await.unwrap();
    assert_eq!(snapshot.skills[0].name, "私有技能");
    assert_eq!(snapshot.skills[0].revision, 1);
    assert!(!snapshot.skills[0].deleted);
    assert_eq!(snapshot.assessments.len(), 1);
    assert_eq!(snapshot.assessments[0].score, 20);
    assert!(plan == f.store.get_learning_plan(&f.owner, &request).await.unwrap());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_skill_and_assessment_quotas_count_tombstones() {
    let f = Fixture::new().await;
    let node = f.skill(&[]).await;
    sqlx::query("INSERT INTO learning_assessments(user_id,id,skill_id,skill_revision,expected_revision,score,assessed_ms) SELECT $1,gen_random_uuid(),$2,1,1,20,i FROM generate_series(1,1000) i")
        .bind(f.owner()).bind(Uuid::parse_str(&node.skill_id).unwrap()).execute(&f.pool).await.unwrap();
    conflict(
        f.store
            .record_assessment(
                &f.owner,
                &key(),
                &AssessmentInput {
                    expected_revision: 1,
                    skill_id: node.skill_id.clone(),
                    skill_revision: 1,
                    score: 80,
                },
            )
            .await,
    );
    f.store
        .delete_skill(&f.owner, &node.skill_id, 1)
        .await
        .unwrap();
    let next = f.skill(&[]).await;
    conflict(
        f.store
            .record_assessment(
                &f.owner,
                &key(),
                &AssessmentInput {
                    expected_revision: 3,
                    skill_id: next.skill_id.clone(),
                    skill_revision: 1,
                    score: 80,
                },
            )
            .await,
    );
    assert!(
        f.store
            .learning_snapshot(&f.owner)
            .await
            .unwrap()
            .assessments
            .is_empty()
    );
    sqlx::query("INSERT INTO learning_skills(user_id,id,revision,name,enabled) SELECT $1,gen_random_uuid(),1,'占位',true FROM generate_series(1,98)")
        .bind(f.owner()).execute(&f.pool).await.unwrap();
    assert_eq!(
        f.store
            .learning_snapshot(&f.owner)
            .await
            .unwrap()
            .skills
            .len(),
        100
    );
    conflict(
        f.store
            .save_skill(&f.owner, &key(), 0, &skill_input("超出上限", &[]))
            .await,
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_oversized_or_future_snapshot_never_creates_partial_plans() {
    let f = Fixture::new().await;
    let node = f.skill(&[]).await;
    let input = f.plan_input(&node).await;
    sqlx::query("INSERT INTO learning_assessments(user_id,id,skill_id,skill_revision,expected_revision,score,assessed_ms) SELECT $1,gen_random_uuid(),$2,1,1,20,i FROM generate_series(1,1001) i")
        .bind(f.owner()).bind(Uuid::parse_str(&node.skill_id).unwrap()).execute(&f.pool).await.unwrap();
    conflict(f.store.create_learning_plan(&f.owner, &key(), &input).await);
    sqlx::query("DELETE FROM learning_assessments WHERE user_id=$1")
        .bind(f.owner())
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO learning_assessments(user_id,id,skill_id,skill_revision,expected_revision,score,assessed_ms) VALUES($1,gen_random_uuid(),$2,1,1,20,9223372036854775807)")
        .bind(f.owner()).bind(Uuid::parse_str(&node.skill_id).unwrap()).execute(&f.pool).await.unwrap();
    assert!(matches!(
        f.store.create_learning_plan(&f.owner, &key(), &input).await,
        Err(StorageError::InvalidData(_))
    ));
    assert!(
        f.store
            .list_learning_plans(&f.owner, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_results_are_atomic_terminal_private_and_do_not_change_assessments() {
    use personal_ai_storage::learning::{TrainingOutcome, TrainingResultInput};
    let f = Fixture::new().await;
    let node = f.skill(&[]).await;
    let input = f.plan_input(&node).await;
    let plan_id = key();
    let plan = f
        .store
        .create_learning_plan(&f.owner, &plan_id, &input)
        .await
        .unwrap();
    let task = &plan.plan.as_ref().unwrap().tasks[0].task_id;
    let completed = TrainingResultInput {
        request_id: key(),
        outcome: TrainingOutcome::Completed,
        note: "私有训练记录".into(),
        actual_minutes: 5,
    };
    let cancelled = TrainingResultInput {
        request_id: key(),
        outcome: TrainingOutcome::Cancelled,
        note: "取消理由".into(),
        actual_minutes: 0,
    };
    assert!(matches!(
        f.store
            .record_training_result(&f.other, &plan_id, task, &completed)
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store
            .record_training_result(&f.owner, &plan_id, &key(), &completed)
            .await,
        Err(StorageError::NotFound)
    ));
    let (a, b) = tokio::join!(
        f.store
            .record_training_result(&f.owner, &plan_id, task, &completed),
        f.store
            .record_training_result(&f.owner, &plan_id, task, &cancelled)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let (winner, loser) = if a.is_ok() {
        (&completed, &cancelled)
    } else {
        (&cancelled, &completed)
    };
    let saved = f
        .store
        .record_training_result(&f.owner, &plan_id, task, winner)
        .await
        .unwrap();
    assert!(saved.plan == plan.plan);
    assert_eq!(saved.digest, plan.digest);
    assert_eq!(saved.results.len(), 1);
    conflict(
        f.store
            .record_training_result(&f.owner, &plan_id, task, loser)
            .await,
    );
    let snapshot = f.store.learning_snapshot(&f.owner).await.unwrap();
    assert_eq!(snapshot.revision, input.expected_revision);
    assert!(snapshot.assessments.is_empty());
    let mut changed = winner.clone();
    changed.note = "不同记录".into();
    conflict(
        f.store
            .record_training_result(&f.owner, &plan_id, task, &changed)
            .await,
    );
    f.store
        .delete_learning_plan(&f.owner, &plan_id)
        .await
        .unwrap();
    let erased = f.store.get_learning_plan(&f.owner, &plan_id).await.unwrap();
    assert!(erased.results.is_empty());
    assert!(erased.plan.is_none());
    conflict(
        f.store
            .record_training_result(&f.owner, &plan_id, task, winner)
            .await,
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)]
async fn learning_results_validate_deduplicate_and_clear_with_sources_and_accounts() {
    use personal_ai_storage::learning::{TrainingOutcome, TrainingResultInput};
    let f = Fixture::new().await;
    let node = f.skill(&[]).await;
    let input = f.plan_input(&node).await;
    let plan_id = key();
    let plan = f
        .store
        .create_learning_plan(&f.owner, &plan_id, &input)
        .await
        .unwrap();
    let task = &plan.plan.as_ref().unwrap().tasks[0].task_id;
    let base = TrainingResultInput {
        request_id: key(),
        outcome: TrainingOutcome::Completed,
        note: "  私有记录\n第二行  ".into(),
        actual_minutes: 1,
    };
    for (note, minutes, outcome) in [
        ("\0".into(), 1, TrainingOutcome::Completed),
        ("字".repeat(2001), 1, TrainingOutcome::Completed),
        (String::new(), 0, TrainingOutcome::Completed),
        (String::new(), 181, TrainingOutcome::Completed),
        (String::new(), 1, TrainingOutcome::Cancelled),
    ] {
        let mut bad = base.clone();
        bad.note = note;
        bad.actual_minutes = minutes;
        bad.outcome = outcome;
        assert!(matches!(
            f.store
                .record_training_result(&f.owner, &plan_id, task, &bad)
                .await,
            Err(StorageError::InvalidData(_))
        ));
    }
    let saved = f
        .store
        .record_training_result(&f.owner, &plan_id, task, &base)
        .await
        .unwrap();
    assert_eq!(saved.results[0].note, "私有记录\n第二行");
    assert!(
        f.store
            .record_training_result(&f.owner, &plan_id, task, &base)
            .await
            .unwrap()
            == saved
    );
    let second_id = key();
    let second = f
        .store
        .create_learning_plan(&f.owner, &second_id, &input)
        .await
        .unwrap();
    let task2 = &second.plan.as_ref().unwrap().tasks[0].task_id;
    conflict(
        f.store
            .record_training_result(&f.owner, &second_id, task2, &base)
            .await,
    );
    assert!(
        f.store
            .get_learning_plan(&f.owner, &second_id)
            .await
            .unwrap()
            .results
            .is_empty()
    );
    // A later skill revision does not rewrite an old plan or prohibit recording historical practice.
    f.store
        .save_skill(&f.owner, &node.skill_id, 1, &skill_input("已修改技能", &[]))
        .await
        .unwrap();
    let mut another = base.clone();
    another.request_id = key();
    f.store
        .record_training_result(&f.owner, &second_id, task2, &another)
        .await
        .unwrap();
    // Force a revision failure after delete cleanup, proving result erasure rolls back as well.
    sqlx::query("UPDATE learning_state SET revision=9223372036854775807 WHERE user_id=$1")
        .bind(f.owner())
        .execute(&f.pool)
        .await
        .unwrap();
    conflict(f.store.delete_skill(&f.owner, &node.skill_id, 2).await);
    assert_eq!(
        f.store
            .get_learning_plan(&f.owner, &plan_id)
            .await
            .unwrap()
            .results
            .len(),
        1
    );
    sqlx::query("UPDATE learning_state SET revision=3 WHERE user_id=$1")
        .bind(f.owner())
        .execute(&f.pool)
        .await
        .unwrap();
    f.store
        .delete_skill(&f.owner, &node.skill_id, 2)
        .await
        .unwrap();
    assert!(
        f.store
            .get_learning_plan(&f.owner, &plan_id)
            .await
            .unwrap()
            .results
            .is_empty()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM learning_results WHERE user_id=$1")
        .bind(f.owner())
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    conflict(
        f.store
            .record_training_result(&f.owner, &plan_id, task, &base)
            .await,
    );
    let fresh = f.skill(&[]).await;
    let id3 = key();
    let plan3 = f
        .store
        .create_learning_plan(&f.owner, &id3, &f.plan_input(&fresh).await)
        .await
        .unwrap();
    f.store
        .record_training_result(
            &f.owner,
            &id3,
            &plan3.plan.as_ref().unwrap().tasks[0].task_id,
            &base,
        )
        .await
        .unwrap();
    f.cleanup().await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM learning_results WHERE user_id=$1")
        .bind(f.owner())
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
