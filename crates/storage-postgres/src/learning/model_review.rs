use super::{
    PostgresStore, StorageError, StorageResult, Uuid, conflict, id, integer, invalid, locked,
    map_error, plans,
};
use personal_ai_domain::UserId;
use personal_ai_learning::model_review::{self, EvidenceFields, ModelReviewPreview, ReviewSource};
use personal_ai_storage::learning::TrainingOutcome;

pub(super) async fn preview(
    store: &PostgresStore,
    owner: Uuid,
    plan: Uuid,
    task: Uuid,
) -> StorageResult<ModelReviewPreview> {
    let mut tx = locked(store, owner).await?;
    let preview = read(&mut tx, owner, plan, task).await?;
    tx.commit().await.map_err(map_error)?;
    Ok(preview)
}
pub(super) async fn read(
    tx: &mut sqlx::PgConnection,
    owner: Uuid,
    plan: Uuid,
    task: Uuid,
) -> StorageResult<ModelReviewPreview> {
    let saved = plans::read(tx, owner, plan).await?;
    let definition = saved.plan.as_ref().ok_or_else(conflict)?;
    if !saved.source_assessments_available {
        return Err(conflict());
    }
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
    let evidence = result
        .evidence
        .as_ref()
        .filter(|e| !e.deleted)
        .ok_or_else(conflict)?;
    let body = evidence.body.as_ref().ok_or_else(conflict)?;
    let name: String = sqlx::query_scalar("SELECT name FROM learning_skills WHERE user_id=$1 AND id=$2 AND revision=$3 AND enabled AND NOT deleted")
        .bind(owner).bind(id(&task.skill_id)?).bind(integer(task.skill_revision)?)
        .fetch_optional(&mut *tx).await.map_err(map_error)?.ok_or_else(conflict)?;
    let preview = model_review::preview(
        &UserId::new(owner.to_string()),
        &ReviewSource {
            plan_id: plan.to_string(),
            task_id: task.task_id.clone(),
            result_request_id: result.request_id.clone(),
            evidence_request_id: evidence.request_id.clone(),
            skill_id: task.skill_id.clone(),
            skill_revision: task.skill_revision,
        },
        &name,
        &task.instructions,
        EvidenceFields {
            explanation: body.explanation.clone(),
            work: body.work.clone(),
            verification: body.verification.clone(),
            limitations: body.limitations.clone(),
        },
    )
    .map_err(|_| invalid())?;
    Ok(preview)
}
