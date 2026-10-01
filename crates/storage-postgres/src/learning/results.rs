use super::{
    PgConnection, PostgresStore, Row, SavedLearningPlan, StorageError, StorageResult, Uuid,
    conflict, id, invalid, locked, map_error, now, number, plans,
};
use personal_ai_storage::learning::{TrainingOutcome, TrainingResult, TrainingResultInput};

pub(super) async fn list(
    tx: &mut PgConnection,
    owner: Uuid,
    plan: Uuid,
) -> StorageResult<Vec<TrainingResult>> {
    let rows = sqlx::query("SELECT r.* FROM learning_results r JOIN learning_tasks t ON t.user_id=r.user_id AND t.id=r.task_id WHERE t.user_id=$1 AND t.request_id=$2 ORDER BY t.ordinal LIMIT 6")
        .bind(owner).bind(plan).fetch_all(tx).await.map_err(map_error)?;
    if rows.len() > 5 {
        return Err(conflict());
    }
    rows.iter()
        .map(|r| {
            Ok(TrainingResult {
                task_id: r.get::<Uuid, _>("task_id").to_string(),
                request_id: r.get::<Uuid, _>("request_id").to_string(),
                outcome: match r.get::<&str, _>("outcome") {
                    "completed" => TrainingOutcome::Completed,
                    "cancelled" => TrainingOutcome::Cancelled,
                    _ => return Err(invalid()),
                },
                note: r.get("note"),
                actual_minutes: u16::try_from(r.get::<i16, _>("actual_minutes"))
                    .map_err(|_| invalid())?,
                recorded_at_unix_ms: number(r.get("recorded_ms"))?,
            })
        })
        .collect()
}
pub(super) async fn record(
    store: &PostgresStore,
    owner: Uuid,
    plan: Uuid,
    task: Uuid,
    mut input: TrainingResultInput,
) -> StorageResult<SavedLearningPlan> {
    let request = id(&input.request_id)?;
    input.note = input.note.trim().to_owned();
    if input.note.chars().count() > 2000
        || input.note.len() > 8000
        || input
            .note
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
        || match input.outcome {
            TrainingOutcome::Completed => !(1..=180).contains(&input.actual_minutes),
            TrainingOutcome::Cancelled => input.actual_minutes != 0,
        }
    {
        return Err(invalid());
    }
    let mut tx = locked(store, owner).await?;
    let saved = plans::read(&mut tx, owner, plan).await?;
    let definition = saved.plan.as_ref().ok_or_else(conflict)?;
    if !definition
        .tasks
        .iter()
        .any(|t| t.task_id == task.to_string())
    {
        return Err(StorageError::NotFound);
    }
    if let Some(old) = saved.results.iter().find(|r| r.task_id == task.to_string()) {
        if old.request_id != request.to_string()
            || old.outcome != input.outcome
            || old.note != input.note
            || old.actual_minutes != input.actual_minutes
        {
            return Err(conflict());
        }
        return Ok(saved);
    }
    let time = now(&mut tx).await?;
    sqlx::query("INSERT INTO learning_results(user_id,task_id,request_id,outcome,note,actual_minutes,recorded_ms) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(owner).bind(task).bind(request).bind(match input.outcome { TrainingOutcome::Completed => "completed", TrainingOutcome::Cancelled => "cancelled" })
        .bind(input.note).bind(i16::try_from(input.actual_minutes).map_err(|_| invalid())?).bind(time).execute(&mut *tx).await.map_err(map_error)?;
    let saved = plans::read(&mut tx, owner, plan).await?;
    tx.commit().await.map_err(map_error)?;
    Ok(saved)
}
