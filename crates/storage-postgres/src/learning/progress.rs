use super::{PostgresStore, Row, StorageResult, Uuid, map_error, now, number, revision};
use personal_ai_learning::planning::TARGET_SCORE;
use personal_ai_storage::learning::LearningProgress;

pub(super) async fn read(store: &PostgresStore, owner: Uuid) -> StorageResult<LearningProgress> {
    let mut tx = store.pool.begin().await.map_err(map_error)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;
    sqlx::query("SET LOCAL statement_timeout = '5s'")
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;
    // Establish the snapshot before sampling the clock, without locking writers.
    sqlx::query("SELECT id FROM users WHERE id=$1")
        .bind(owner)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_error)?;
    let time = now(&mut tx).await?;
    let start = time / 86_400_000 * 86_400_000;
    let end = start.checked_add(86_400_000).ok_or_else(super::invalid)?;
    let version = revision(&mut tx, owner).await?;
    let row = sqlx::query(include_str!("progress.sql"))
        .bind(owner)
        .bind(time)
        .bind(start)
        .bind(end)
        .bind(i16::from(TARGET_SCORE))
        .bind(super::integer(version)?)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_error)?;
    let saved = LearningProgress {
        timezone: "UTC",
        as_of_unix_ms: number(time)?,
        day_start_unix_ms: number(start)?,
        day_end_unix_ms: number(end)?,
        revision: version,
        target_score: TARGET_SCORE,
        enabled_skills: number(row.get("enabled_skills"))?,
        assessed_skills: number(row.get("assessed_skills"))?,
        target_reached_skills: number(row.get("target_reached_skills"))?,
        ready_plans: number(row.get("ready_plans"))?,
        historical_plans: number(row.get("historical_plans"))?,
        pending_tasks: number(row.get("pending_tasks"))?,
        completed_tasks: number(row.get("completed_tasks"))?,
        cancelled_tasks: number(row.get("cancelled_tasks"))?,
        completed_today: number(row.get("completed_today"))?,
        cancelled_today: number(row.get("cancelled_today"))?,
        recorded_minutes_today: number(row.get("recorded_minutes_today"))?,
    };
    tx.commit().await.map_err(map_error)?;
    Ok(saved)
}
