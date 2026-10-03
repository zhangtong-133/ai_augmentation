use super::{
    PgConnection, PostgresStore, Row, SavedLearningPlan, StorageError, StorageResult, Uuid,
    conflict, id, integer, invalid, locked, map_error, now, number, plans, skills,
};
use personal_ai_storage::learning::{
    AssessmentInput,
    review::{
        RUBRIC_VERSION, ReviewBody, ReviewConfirmation, ReviewInput, ReviewStatus, TrainingReview,
    },
};

pub(super) async fn read(
    tx: &mut PgConnection,
    owner: Uuid,
    task: Uuid,
) -> StorageResult<Option<TrainingReview>> {
    let row = sqlx::query("SELECT * FROM learning_reviews WHERE user_id=$1 AND task_id=$2")
        .bind(owner)
        .bind(task)
        .fetch_optional(tx)
        .await
        .map_err(map_error)?;
    row.map(|r| {
        Ok(TrainingReview {
            request_id: r.get::<Uuid, _>("request_id").to_string(),
            evidence_request_id: r.get::<Uuid, _>("evidence_request_id").to_string(),
            rubric_version: r.get("rubric_version"),
            body: r
                .get::<Option<serde_json::Value>, _>("body")
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| invalid())?,
            status: match r.get::<&str, _>("status") {
                "pending" => ReviewStatus::Pending,
                "confirmed" => ReviewStatus::Confirmed,
                "invalidated" => ReviewStatus::Invalidated,
                _ => return Err(invalid()),
            },
            created_at_unix_ms: number(r.get("created_ms"))?,
            confirmation_request_id: r
                .get::<Option<Uuid>, _>("confirmation_request_id")
                .map(|v| v.to_string()),
            assessment_id: r
                .get::<Option<Uuid>, _>("assessment_id")
                .map(|v| v.to_string()),
            expected_revision: r
                .get::<Option<i64>, _>("expected_revision")
                .map(number)
                .transpose()?,
            confirmed_score: r
                .get::<Option<i16>, _>("confirmed_score")
                .map(|v| u8::try_from(v).map_err(|_| invalid()))
                .transpose()?,
        })
    })
    .transpose()
}
fn normalize(mut body: ReviewBody) -> StorageResult<ReviewBody> {
    for dim in [
        &mut body.explanation,
        &mut body.work,
        &mut body.verification,
        &mut body.limitations,
    ] {
        dim.reason = dim.reason.trim().to_owned();
        if dim.reason.is_empty()
            || dim.reason.chars().count() > 500
            || dim.reason.len() > 2000
            || dim
                .reason
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return Err(invalid());
        }
    }
    Ok(body)
}
fn source(
    saved: &SavedLearningPlan,
    task: Uuid,
) -> StorageResult<(
    &personal_ai_learning::planning::TrainingTask,
    &personal_ai_storage::learning::TrainingEvidence,
)> {
    let task = saved
        .plan
        .as_ref()
        .ok_or_else(conflict)?
        .tasks
        .iter()
        .find(|t| t.task_id == task.to_string())
        .ok_or(StorageError::NotFound)?;
    let evidence = saved
        .results
        .iter()
        .find(|r| r.task_id == task.task_id)
        .and_then(|r| r.evidence.as_ref())
        .filter(|e| !e.deleted && e.body.is_some())
        .ok_or_else(conflict)?;
    Ok((task, evidence))
}
pub(super) async fn save(
    store: &PostgresStore,
    owner: Uuid,
    plan: Uuid,
    task: Uuid,
    input: ReviewInput,
) -> StorageResult<SavedLearningPlan> {
    let request = id(&input.request_id)?;
    let evidence_id = id(&input.evidence_request_id)?;
    let body = normalize(input.body)?;
    let mut tx = locked(store, owner).await?;
    let saved = plans::read(&mut tx, owner, plan).await?;
    let (definition, evidence) = source(&saved, task)?;
    if evidence.request_id != evidence_id.to_string() {
        return Err(conflict());
    }
    if let Some(old) = &evidence.review {
        if old.status == ReviewStatus::Invalidated
            || old.request_id != request.to_string()
            || old.body.as_ref() != Some(&body)
        {
            return Err(conflict());
        }
        return Ok(saved);
    }
    if !saved.source_assessments_available {
        return Err(conflict());
    }
    let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM learning_skills WHERE user_id=$1 AND id=$2 AND revision=$3 AND enabled AND NOT deleted)")
        .bind(owner).bind(id(&definition.skill_id)?).bind(integer(definition.skill_revision)?).fetch_one(&mut *tx).await.map_err(map_error)?;
    if !current {
        return Err(conflict());
    }
    let time = now(&mut tx).await?;
    sqlx::query("INSERT INTO learning_reviews(user_id,task_id,request_id,evidence_request_id,rubric_version,body,status,created_ms) VALUES($1,$2,$3,$4,$5,$6,'pending',$7)")
        .bind(owner).bind(task).bind(request).bind(evidence_id).bind(RUBRIC_VERSION).bind(serde_json::to_value(body).map_err(|_|invalid())?).bind(time).execute(&mut *tx).await.map_err(map_error)?;
    let saved = plans::read(&mut tx, owner, plan).await?;
    tx.commit().await.map_err(map_error)?;
    Ok(saved)
}
pub(super) async fn confirm(
    store: &PostgresStore,
    owner: Uuid,
    plan: Uuid,
    task: Uuid,
    input: ReviewConfirmation,
) -> StorageResult<SavedLearningPlan> {
    let request = id(&input.request_id)?;
    let review_id = id(&input.review_request_id)?;
    integer(input.expected_revision)?;
    if input.score > 100 {
        return Err(invalid());
    }
    let mut tx = locked(store, owner).await?;
    let saved = plans::read(&mut tx, owner, plan).await?;
    let (definition, evidence) = source(&saved, task)?;
    let review = evidence.review.as_ref().ok_or_else(conflict)?;
    if review.request_id != review_id.to_string() || review.status == ReviewStatus::Invalidated {
        return Err(conflict());
    }
    if review.status == ReviewStatus::Confirmed {
        if review.confirmation_request_id.as_deref() != Some(request.to_string().as_str())
            || review.confirmed_score != Some(input.score)
            || review.expected_revision != Some(input.expected_revision)
        {
            return Err(conflict());
        }
        return Ok(saved);
    }
    if !review.body.as_ref().is_some_and(ReviewBody::supported) {
        return Err(conflict());
    }
    if !saved.source_assessments_available {
        return Err(conflict());
    }
    let assessment_id = Uuid::new_v4();
    skills::assess_locked(
        &mut tx,
        owner,
        assessment_id,
        AssessmentInput {
            expected_revision: input.expected_revision,
            skill_id: definition.skill_id.clone(),
            skill_revision: definition.skill_revision,
            score: input.score,
        },
    )
    .await?;
    sqlx::query("UPDATE learning_reviews SET status='confirmed',confirmation_request_id=$3,assessment_id=$4,expected_revision=$5,confirmed_score=$6 WHERE user_id=$1 AND task_id=$2")
        .bind(owner).bind(task).bind(request).bind(assessment_id).bind(integer(input.expected_revision)?).bind(i16::from(input.score)).execute(&mut *tx).await.map_err(map_error)?;
    let saved = plans::read(&mut tx, owner, plan).await?;
    tx.commit().await.map_err(map_error)?;
    Ok(saved)
}
