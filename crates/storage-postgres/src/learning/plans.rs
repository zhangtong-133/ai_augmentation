use super::{
    Digest, LEARNING_VERSION, LearningPlan, LearningPlanInput, LearningPlanPage,
    LearningPlanStatus, LearningPlanSummary, LearningRequest, PgConnection, PgRow, PostgresStore,
    Row, SavedLearningPlan, Sha256, StorageError, StorageResult, UserId, Uuid, conflict, graph, id,
    integer, invalid, locked, map_error, now, number, plan_learning, ratings, revision,
};

fn status(value: &str) -> StorageResult<LearningPlanStatus> {
    match value {
        "ready" => Ok(LearningPlanStatus::Ready),
        "invalidated" => Ok(LearningPlanStatus::Invalidated),
        "deleted" => Ok(LearningPlanStatus::Deleted),
        _ => Err(invalid()),
    }
}
fn normalize(mut input: LearningPlanInput) -> StorageResult<(LearningPlanInput, String)> {
    integer(input.expected_revision)?;
    if !(5..=180).contains(&input.budget_minutes)
        || input.goal_skill_ids.is_empty()
        || input.goal_skill_ids.len() > 10
    {
        return Err(invalid());
    }
    input.goal_skill_ids = input
        .goal_skill_ids
        .iter()
        .map(|v| Ok(id(v)?.to_string()))
        .collect::<StorageResult<_>>()?;
    input.goal_skill_ids.sort();
    if input.goal_skill_ids.windows(2).any(|w| w[0] == w[1]) {
        return Err(invalid());
    }
    let bytes = serde_json::to_vec(&(
        input.expected_revision,
        input.budget_minutes,
        &input.goal_skill_ids,
    ))
    .map_err(|_| invalid())?;
    let digest = format!("{:x}", Sha256::digest(bytes));
    Ok((input, digest))
}
fn record(row: &PgRow) -> StorageResult<SavedLearningPlan> {
    let owner = row.get::<Uuid, _>("user_id").to_string();
    let saved = SavedLearningPlan {
        source_assessments_available: false,
        results: Vec::new(),
        request_id: row.get::<Uuid, _>("request_id").to_string(),
        snapshot_revision: number(row.get("snapshot_revision"))?,
        created_at_unix_ms: number(row.get("created_ms"))?,
        status: status(row.get("status"))?,
        digest: row.get("digest"),
        plan: row
            .get::<Option<serde_json::Value>, _>("plan")
            .map(serde_json::from_value::<LearningPlan>)
            .transpose()
            .map_err(|_| invalid())?,
    };
    if let Some(plan) = &saved.plan {
        let (_, request_digest) = normalize(LearningPlanInput {
            expected_revision: saved.snapshot_revision,
            budget_minutes: plan.budget_minutes,
            goal_skill_ids: plan.goal_skill_ids.clone(),
        })?;
        if plan.version != LEARNING_VERSION
            || plan.user_id != owner
            || plan.request_id != saved.request_id
            || plan.as_of_unix_ms != saved.created_at_unix_ms
            || plan.digest().map_err(|_| invalid())? != saved.digest
            || request_digest != row.get::<String, _>("request_digest")
        {
            return Err(conflict());
        }
    }
    Ok(saved)
}
async fn verify_tasks(
    tx: &mut PgConnection,
    owner: Uuid,
    saved: &SavedLearningPlan,
) -> StorageResult<()> {
    let rows=sqlx::query("SELECT id,ordinal,status,task FROM learning_tasks WHERE user_id=$1 AND request_id=$2 ORDER BY ordinal LIMIT 6").bind(owner).bind(id(&saved.request_id)?).fetch_all(tx).await.map_err(map_error)?;
    if rows.len() > 5 {
        return Err(conflict());
    }
    if let Some(plan) = &saved.plan {
        if rows.len() != plan.tasks.len() {
            return Err(conflict());
        }
        for (ordinal, (row, task)) in rows.iter().zip(&plan.tasks).enumerate() {
            if row.get::<Uuid, _>("id").to_string() != task.task_id
                || row.get::<i16, _>("ordinal") != i16::try_from(ordinal).map_err(|_| invalid())?
                || row.get::<String, _>("status") != "planned"
                || row.get::<Option<serde_json::Value>, _>("task")
                    != Some(serde_json::to_value(task).map_err(|_| invalid())?)
            {
                return Err(conflict());
            }
        }
    } else {
        let expected = if saved.status == LearningPlanStatus::Deleted {
            "deleted"
        } else {
            "invalidated"
        };
        if rows.iter().any(|r| {
            r.get::<Option<serde_json::Value>, _>("task").is_some()
                || r.get::<String, _>("status") != expected
        }) {
            return Err(conflict());
        }
    }
    Ok(())
}
pub(super) async fn read(
    tx: &mut PgConnection,
    owner: Uuid,
    key: Uuid,
) -> StorageResult<SavedLearningPlan> {
    let row = sqlx::query("SELECT * FROM learning_plans WHERE user_id=$1 AND request_id=$2")
        .bind(owner)
        .bind(key)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_error)?;
    let mut saved = record(&row)?;
    verify_tasks(tx, owner, &saved).await?;
    if let Some(plan) = &saved.plan {
        let ids = plan
            .evaluations
            .iter()
            .filter_map(|e| e.assessment_id.as_deref())
            .map(id)
            .collect::<StorageResult<Vec<_>>>()?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM learning_assessments WHERE user_id=$1 AND id=ANY($2) AND score IS NOT NULL")
            .bind(owner).bind(&ids).fetch_one(&mut *tx).await.map_err(map_error)?;
        saved.source_assessments_available =
            usize::try_from(count).map_err(|_| invalid())? == ids.len();
    }
    saved.results = super::results::list(tx, owner, key).await?;
    if saved.plan.is_none() && !saved.results.is_empty() {
        return Err(conflict());
    }
    Ok(saved)
}
pub(super) async fn create(
    store: &PostgresStore,
    owner: Uuid,
    key: Uuid,
    input: LearningPlanInput,
) -> StorageResult<SavedLearningPlan> {
    let (input, request_digest) = normalize(input)?;
    let mut tx = locked(store, owner).await?;
    if let Some(row) =
        sqlx::query("SELECT request_digest FROM learning_plans WHERE user_id=$1 AND request_id=$2")
            .bind(owner)
            .bind(key)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_error)?
    {
        if row.get::<String, _>("request_digest") != request_digest {
            return Err(conflict());
        }
        return read(&mut tx, owner, key).await;
    }
    if revision(&mut tx, owner).await? != input.expected_revision {
        return Err(conflict());
    }
    let time = now(&mut tx).await?;
    let start = time / 86_400_000 * 86_400_000;
    let limits=sqlx::query("SELECT count(*) AS total,count(*) FILTER(WHERE created_ms>=$2) AS daily FROM learning_plans WHERE user_id=$1")
        .bind(owner).bind(start).fetch_one(&mut *tx).await.map_err(map_error)?;
    if limits.get::<i64, _>("total") >= 1000 || limits.get::<i64, _>("daily") >= 10 {
        return Err(conflict());
    }
    let skills = graph(&mut tx, owner).await?;
    let assessments = ratings(&mut tx, owner).await?;
    let request = LearningRequest {
        request_id: key.to_string(),
        as_of_unix_ms: number(time)?,
        budget_minutes: input.budget_minutes,
        goal_skill_ids: input.goal_skill_ids,
    };
    let plan = plan_learning(
        &UserId::new(owner.to_string()),
        &request,
        &skills,
        &assessments,
    )
    .map_err(|_| invalid())?;
    let digest = plan.digest().map_err(|_| invalid())?;
    sqlx::query("INSERT INTO learning_plans(user_id,request_id,snapshot_revision,request_digest,digest,created_ms,status,plan) VALUES($1,$2,$3,$4,$5,$6,'ready',$7)")
        .bind(owner).bind(key).bind(integer(input.expected_revision)?).bind(request_digest).bind(digest).bind(time).bind(serde_json::to_value(&plan).map_err(|_|invalid())?).execute(&mut *tx).await.map_err(map_error)?;
    for skill in skills {
        sqlx::query(
            "INSERT INTO learning_plan_sources(user_id,request_id,skill_id) VALUES($1,$2,$3)",
        )
        .bind(owner)
        .bind(key)
        .bind(id(&skill.skill_id)?)
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;
    }
    for (ordinal, task) in plan.tasks.iter().enumerate() {
        sqlx::query("INSERT INTO learning_tasks(user_id,request_id,id,ordinal,status,task) VALUES($1,$2,$3,$4,'planned',$5)")
            .bind(owner).bind(key).bind(id(&task.task_id)?).bind(i16::try_from(ordinal).map_err(|_|invalid())?).bind(serde_json::to_value(task).map_err(|_|invalid())?).execute(&mut *tx).await.map_err(map_error)?;
    }
    let saved = read(&mut tx, owner, key).await?;
    tx.commit().await.map_err(map_error)?;
    Ok(saved)
}
pub(super) async fn delete(store: &PostgresStore, owner: Uuid, key: Uuid) -> StorageResult<()> {
    let mut tx = locked(store, owner).await?;
    let result = sqlx::query(
        "UPDATE learning_plans SET status='deleted',plan=NULL WHERE user_id=$1 AND request_id=$2",
    )
    .bind(owner)
    .bind(key)
    .execute(&mut *tx)
    .await
    .map_err(map_error)?;
    if result.rows_affected() == 0 {
        return Err(StorageError::NotFound);
    }
    sqlx::query(
        "UPDATE learning_tasks SET status='deleted',task=NULL WHERE user_id=$1 AND request_id=$2",
    )
    .bind(owner)
    .bind(key)
    .execute(&mut *tx)
    .await
    .map_err(map_error)?;
    sqlx::query("DELETE FROM learning_plan_sources WHERE user_id=$1 AND request_id=$2")
        .bind(owner)
        .bind(key)
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;
    tx.commit().await.map_err(map_error)
}
pub(super) async fn list(
    store: &PostgresStore,
    owner: Uuid,
    after: Option<Uuid>,
) -> StorageResult<LearningPlanPage> {
    let rows=sqlx::query("SELECT request_id,created_ms,status FROM learning_plans WHERE user_id=$1 AND ($2::uuid IS NULL OR request_id>$2) ORDER BY request_id LIMIT 21")
        .bind(owner).bind(after).fetch_all(&store.pool).await.map_err(map_error)?;
    let items = rows
        .iter()
        .take(20)
        .map(|r| {
            Ok(LearningPlanSummary {
                request_id: r.get::<Uuid, _>("request_id").to_string(),
                created_at_unix_ms: number(r.get("created_ms"))?,
                status: status(r.get("status"))?,
            })
        })
        .collect::<StorageResult<Vec<_>>>()?;
    let next_cursor =
        (rows.len() > 20).then(|| items.last().expect("full learning page").request_id.clone());
    Ok(LearningPlanPage { items, next_cursor })
}
