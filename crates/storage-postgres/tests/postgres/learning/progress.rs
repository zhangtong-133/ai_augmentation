use super::*;
use personal_ai_storage::learning::{TrainingOutcome, TrainingResultInput};
async fn result(f: &Fixture, plan: &str, task: &str, outcome: TrainingOutcome, minutes: u16) {
    f.store
        .record_training_result(
            &f.owner,
            plan,
            task,
            &TrainingResultInput {
                request_id: key(),
                outcome,
                note: "private-progress-note".into(),
                actual_minutes: minutes,
            },
        )
        .await
        .unwrap();
}
async fn plan(f: &Fixture, node: &SkillNode) -> (String, String) {
    let id = key();
    let saved = f
        .store
        .create_learning_plan(&f.owner, &id, &f.plan_input(node).await)
        .await
        .unwrap();
    (id, saved.plan.unwrap().tasks[0].task_id.clone())
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn progress_counts_current_assessments_and_retained_results_without_mutating() {
    let f = Fixture::new().await;
    let empty = f.store.learning_progress(&f.owner).await.unwrap();
    assert_eq!(empty.enabled_skills, 0);
    assert_eq!(empty.pending_tasks, 0);
    assert_eq!(empty.recorded_minutes_today, 0);
    let node = f.skill(&[]).await;
    let (first, task1) = plan(&f, &node).await;
    let (second, task2) = plan(&f, &node).await;
    result(&f, &first, &task1, TrainingOutcome::Completed, 12).await;
    result(&f, &second, &task2, TrainingOutcome::Cancelled, 0).await;
    let p = f.store.learning_progress(&f.owner).await.unwrap();
    assert_eq!(p.enabled_skills, 1);
    assert_eq!(p.assessed_skills, 0);
    assert_eq!(p.completed_tasks, 1);
    assert_eq!(p.cancelled_tasks, 1);
    assert_eq!(p.completed_today, 1);
    assert_eq!(p.cancelled_today, 1);
    assert_eq!(p.recorded_minutes_today, 12);
    assert_eq!(p.pending_tasks, 0);
    assert_eq!(p.revision, 1);
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
        .await
        .unwrap();
    let p = f.store.learning_progress(&f.owner).await.unwrap();
    assert_eq!(p.assessed_skills, 1);
    assert_eq!(p.target_reached_skills, 1);
    assert_eq!(p.target_score, 70);
    assert_eq!(p.historical_plans, 2);
    sqlx::query("UPDATE learning_assessments SET assessed_ms=assessed_ms-10 WHERE user_id=$1")
        .bind(f.owner())
        .execute(&f.pool)
        .await
        .unwrap();
    // A second current-version rating is the newest evidence, not the highest score.
    sqlx::query("INSERT INTO learning_assessments(user_id,id,skill_id,skill_revision,expected_revision,score,assessed_ms) SELECT $1,gen_random_uuid(),$2,1,2,30,max(assessed_ms)+1 FROM learning_assessments WHERE user_id=$1").bind(f.owner()).bind(Uuid::parse_str(&node.skill_id).unwrap()).execute(&f.pool).await.unwrap();
    sqlx::query("UPDATE learning_state SET revision=3 WHERE user_id=$1")
        .bind(f.owner())
        .execute(&f.pool)
        .await
        .unwrap();
    let p = f.store.learning_progress(&f.owner).await.unwrap();
    assert_eq!(p.assessed_skills, 1);
    assert_eq!(p.target_reached_skills, 0);
    f.store
        .save_skill(&f.owner, &node.skill_id, 1, &skill_input("新版本", &[]))
        .await
        .unwrap();
    let p = f.store.learning_progress(&f.owner).await.unwrap();
    assert_eq!(p.assessed_skills, 0);
    assert_eq!(p.enabled_skills, 1);
    assert_eq!(p.completed_tasks, 1);
    let isolated = f.store.learning_progress(&f.other).await.unwrap();
    assert_eq!(isolated.enabled_skills, 0);
    assert_eq!(isolated.ready_plans, 0);
    assert_eq!(isolated.completed_today, 0);
    f.store
        .delete_learning_plan(&f.owner, &first)
        .await
        .unwrap();
    let p = f.store.learning_progress(&f.owner).await.unwrap();
    assert_eq!(p.completed_tasks, 0);
    assert_eq!(p.recorded_minutes_today, 0);
    assert_eq!(p.cancelled_tasks, 1);
    f.store
        .delete_skill(&f.owner, &node.skill_id, 2)
        .await
        .unwrap();
    let p = f.store.learning_progress(&f.owner).await.unwrap();
    assert_eq!(p.enabled_skills, 0);
    assert_eq!(p.ready_plans, 0);
    assert_eq!(p.cancelled_tasks, 0);
    assert_eq!(p.revision, f.version().await);
    assert!(matches!(
        f.store.learning_progress(&UserId::new(key())).await,
        Err(StorageError::NotFound)
    ));
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn progress_uses_half_open_utc_days_and_excludes_future_results() {
    let f = Fixture::new().await;
    let node = f.skill(&[]).await;
    let before = f.store.learning_progress(&f.owner).await.unwrap();
    let start = i64::try_from(before.day_start_unix_ms).unwrap();
    let end = i64::try_from(before.day_end_unix_ms).unwrap();
    assert_eq!(end - start, 86_400_000);
    assert_eq!(start % 86_400_000, 0);
    for (time, minutes) in [(start - 1, 5), (start, 7), (end, 11), (end - 1, 13)] {
        let (id, task) = plan(&f, &node).await;
        result(&f, &id, &task, TrainingOutcome::Completed, minutes).await;
        sqlx::query("UPDATE learning_results SET recorded_ms=$3 WHERE user_id=$1 AND task_id=$2")
            .bind(f.owner())
            .bind(Uuid::parse_str(&task).unwrap())
            .bind(time)
            .execute(&f.pool)
            .await
            .unwrap();
    }
    let after = f.store.learning_progress(&f.owner).await.unwrap();
    if after.day_start_unix_ms == before.day_start_unix_ms
        && after.as_of_unix_ms < before.day_end_unix_ms - 1
    {
        assert_eq!(after.completed_today, 1);
        assert_eq!(after.recorded_minutes_today, 7);
    } else {
        // Still assert the precise window if this test straddles UTC midnight.
        let expected = [(start - 1, 5u64), (start, 7), (end, 11), (end - 1, 13)]
            .into_iter()
            .filter(|(at, _)| {
                u64::try_from(*at).unwrap() >= after.day_start_unix_ms
                    && u64::try_from(*at).unwrap() < after.day_end_unix_ms
                    && u64::try_from(*at).unwrap() <= after.as_of_unix_ms
            })
            .collect::<Vec<_>>();
        assert_eq!(
            after.completed_today,
            u64::try_from(expected.len()).unwrap()
        );
        assert_eq!(
            after.recorded_minutes_today,
            expected.iter().map(|(_, n)| n).sum::<u64>()
        );
    }
    assert_eq!(after.completed_tasks, 4);
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn progress_keeps_one_snapshot_during_result_deletion() {
    let f = Fixture::new().await;
    let node = f.skill(&[]).await;
    let (id, task) = plan(&f, &node).await;
    result(&f, &id, &task, TrainingOutcome::Completed, 20).await;
    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query("UPDATE learning_plans SET status='deleted',plan=NULL WHERE user_id=$1")
        .bind(f.owner())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE learning_tasks SET status='deleted',task=NULL WHERE user_id=$1")
        .bind(f.owner())
        .execute(&mut *tx)
        .await
        .unwrap();
    let reads = async {
        for _ in 0..8 {
            let p = f.store.learning_progress(&f.owner).await.unwrap();
            assert_eq!(p.ready_plans, p.completed_tasks);
            assert_eq!(p.recorded_minutes_today, 20 * p.completed_today);
            assert_eq!(p.pending_tasks, 0);
        }
    };
    let ((), done) = tokio::join!(reads, tx.commit());
    done.unwrap();
    assert_eq!(
        f.store
            .learning_progress(&f.owner)
            .await
            .unwrap()
            .completed_tasks,
        0
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn progress_excludes_disabled_skills_and_does_not_count_historical_ratings() {
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
                score: 100,
            },
        )
        .await
        .unwrap();
    f.store
        .save_skill(
            &f.owner,
            &node.skill_id,
            1,
            &SkillInput {
                name: node.name.clone(),
                enabled: false,
                prerequisite_ids: vec![],
            },
        )
        .await
        .unwrap();
    let p = f.store.learning_progress(&f.owner).await.unwrap();
    assert_eq!(p.enabled_skills, 0);
    assert_eq!(p.assessed_skills, 0);
    assert_eq!(p.target_reached_skills, 0);
    f.cleanup().await;
}
