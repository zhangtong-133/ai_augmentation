use super::{
    PgConnection, PostgresStore, Row, SavedLearningPlan, StorageError, StorageResult, Uuid,
    conflict, id, integer, invalid, locked, map_error, now, number, plans,
};
use personal_ai_storage::learning::{TrainingEvidence, TrainingEvidenceBody, TrainingOutcome};

pub(super) async fn read(
    tx: &mut PgConnection,
    owner: Uuid,
    task: Uuid,
) -> StorageResult<Option<TrainingEvidence>> {
    let row = sqlx::query("SELECT * FROM learning_evidence WHERE user_id=$1 AND task_id=$2")
        .bind(owner)
        .bind(task)
        .fetch_optional(tx)
        .await
        .map_err(map_error)?;
    row.map(|r| {
        Ok(TrainingEvidence {
            request_id: r.get::<Uuid, _>("request_id").to_string(),
            created_at_unix_ms: number(r.get("created_ms"))?,
            deleted: r.get("deleted"),
            body: r
                .get::<Option<serde_json::Value>, _>("body")
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| invalid())?,
        })
    })
    .transpose()
}
fn normalize(mut body: TrainingEvidenceBody) -> StorageResult<TrainingEvidenceBody> {
    for text in [
        &mut body.explanation,
        &mut body.work,
        &mut body.verification,
        &mut body.limitations,
    ] {
        *text = text.trim().to_owned();
        if text.is_empty()
            || text.chars().count() > 2000
            || text.len() > 8000
            || text
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return Err(invalid());
        }
    }
    Ok(body)
}
pub(super) async fn save(
    store: &PostgresStore,
    owner: Uuid,
    plan: Uuid,
    task: Uuid,
    request: Uuid,
    body: TrainingEvidenceBody,
) -> StorageResult<SavedLearningPlan> {
    let body = normalize(body)?;
    let mut tx = locked(store, owner).await?;
    let saved = plans::read(&mut tx, owner, plan).await?;
    let definition = saved.plan.as_ref().ok_or_else(conflict)?;
    let task = definition
        .tasks
        .iter()
        .find(|t| t.task_id == task.to_string())
        .ok_or(StorageError::NotFound)?;
    let result = saved
        .results
        .iter()
        .find(|r| r.task_id == task.task_id)
        .ok_or_else(conflict)?;
    if result.outcome != TrainingOutcome::Completed {
        return Err(conflict());
    }
    if let Some(old) = &result.evidence {
        if old.deleted || old.request_id != request.to_string() || old.body.as_ref() != Some(&body)
        {
            return Err(conflict());
        }
        return Ok(saved);
    }
    let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM learning_skills WHERE user_id=$1 AND id=$2 AND revision=$3 AND enabled AND NOT deleted)")
        .bind(owner).bind(id(&task.skill_id)?).bind(integer(task.skill_revision)?).fetch_one(&mut *tx).await.map_err(map_error)?;
    if !current {
        return Err(conflict());
    }
    let time = now(&mut tx).await?;
    sqlx::query("INSERT INTO learning_evidence(user_id,task_id,request_id,body,created_ms) VALUES($1,$2,$3,$4,$5)")
        .bind(owner).bind(id(&task.task_id)?).bind(request).bind(serde_json::to_value(body).map_err(|_| invalid())?).bind(time)
        .execute(&mut *tx).await.map_err(map_error)?;
    let saved = plans::read(&mut tx, owner, plan).await?;
    tx.commit().await.map_err(map_error)?;
    Ok(saved)
}
pub(super) async fn delete(
    store: &PostgresStore,
    owner: Uuid,
    plan: Uuid,
    task: Uuid,
    request: Uuid,
) -> StorageResult<SavedLearningPlan> {
    let mut tx = locked(store, owner).await?;
    let saved = plans::read(&mut tx, owner, plan).await?;
    saved.plan.as_ref().ok_or_else(conflict)?;
    let old = saved
        .results
        .iter()
        .find(|r| r.task_id == task.to_string())
        .and_then(|r| r.evidence.as_ref())
        .ok_or(StorageError::NotFound)?;
    if old.request_id != request.to_string() {
        return Err(conflict());
    }
    sqlx::query(
        "UPDATE learning_evidence SET deleted=TRUE,body=NULL WHERE user_id=$1 AND task_id=$2",
    )
    .bind(owner)
    .bind(task)
    .execute(&mut *tx)
    .await
    .map_err(map_error)?;
    let saved = plans::read(&mut tx, owner, plan).await?;
    tx.commit().await.map_err(map_error)?;
    Ok(saved)
}
