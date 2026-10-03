use crate::{
    PostgresStore, map_error,
    schedules::{lock_owner, now},
};
use personal_ai_domain::UserId;
use personal_ai_learning::planning::{
    LEARNING_VERSION, LearningPlan, LearningRequest, SelfAssessment, SkillNode,
    normalize_skill_graph, plan_learning,
};
use personal_ai_storage::learning::model_authorization::{
    ModelApproval, ModelAuthorization, ModelAuthorizationInput, ModelAuthorizationPage,
};
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    learning::{
        AssessmentInput, LearningPlanInput, LearningPlanPage, LearningPlanStatus,
        LearningPlanSummary, LearningSnapshot, LearningStore, SavedLearningPlan, SkillInput,
    },
};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;
mod evidence;
mod model_authorization;
mod model_review;
mod plans;
mod progress;
mod results;
mod reviews;
mod skills;
fn invalid() -> StorageError {
    StorageError::InvalidData("invalid learning input".into())
}
fn conflict() -> StorageError {
    StorageError::Conflict("learning version, snapshot or quota changed".into())
}
pub(super) fn id(value: &str) -> StorageResult<Uuid> {
    let id = Uuid::parse_str(value).map_err(|_| invalid())?;
    if id.is_nil() {
        return Err(invalid());
    }
    Ok(id)
}
fn number(value: i64) -> StorageResult<u64> {
    u64::try_from(value).map_err(|_| invalid())
}
fn integer(value: u64) -> StorageResult<i64> {
    i64::try_from(value).map_err(|_| invalid())
}
async fn locked(store: &PostgresStore, owner: Uuid) -> StorageResult<Transaction<'_, Postgres>> {
    let mut tx = store.pool.begin().await.map_err(map_error)?;
    lock_owner(&mut tx, owner).await?;
    Ok(tx)
}
async fn revision(tx: &mut PgConnection, owner: Uuid) -> StorageResult<u64> {
    let n: Option<i64> = sqlx::query_scalar("SELECT revision FROM learning_state WHERE user_id=$1")
        .bind(owner)
        .fetch_optional(tx)
        .await
        .map_err(map_error)?;
    number(n.unwrap_or(0))
}
async fn bump(tx: &mut PgConnection, owner: Uuid) -> StorageResult<()> {
    let n=sqlx::query("INSERT INTO learning_state(user_id,revision) VALUES($1,1) ON CONFLICT(user_id) DO UPDATE SET revision=learning_state.revision+1 WHERE learning_state.revision<9223372036854775807")
        .bind(owner).execute(tx).await.map_err(map_error)?;
    if n.rows_affected() != 1 {
        return Err(conflict());
    }
    Ok(())
}
async fn graph(tx: &mut PgConnection, owner: Uuid) -> StorageResult<Vec<SkillNode>> {
    let rows=sqlx::query("SELECT s.*,ARRAY(SELECT prerequisite_id FROM learning_edges e WHERE e.user_id=s.user_id AND e.skill_id=s.id ORDER BY prerequisite_id LIMIT 9) AS parents FROM learning_skills s WHERE user_id=$1 ORDER BY id LIMIT 101")
        .bind(owner).fetch_all(tx).await.map_err(map_error)?;
    if rows.len() > 100 {
        return Err(conflict());
    }
    let nodes = rows
        .into_iter()
        .map(|r| {
            Ok(SkillNode {
                user_id: owner.to_string(),
                skill_id: r.get::<Uuid, _>("id").to_string(),
                revision: number(r.get("revision"))?,
                name: r.get("name"),
                enabled: r.get("enabled"),
                deleted: r.get("deleted"),
                prerequisite_ids: r
                    .get::<Vec<Uuid>, _>("parents")
                    .into_iter()
                    .map(|p| p.to_string())
                    .collect(),
            })
        })
        .collect::<StorageResult<Vec<_>>>()?;
    normalize_skill_graph(&UserId::new(owner.to_string()), &nodes).map_err(|_| invalid())
}
fn rating(row: &PgRow) -> StorageResult<SelfAssessment> {
    Ok(SelfAssessment {
        user_id: row.get::<Uuid, _>("user_id").to_string(),
        assessment_id: row.get::<Uuid, _>("id").to_string(),
        skill_id: row.get::<Uuid, _>("skill_id").to_string(),
        skill_revision: number(row.get("skill_revision"))?,
        score: u8::try_from(row.get::<Option<i16>, _>("score").ok_or_else(conflict)?)
            .map_err(|_| invalid())?,
        assessed_at_unix_ms: number(row.get("assessed_ms"))?,
    })
}
async fn ratings(tx: &mut PgConnection, owner: Uuid) -> StorageResult<Vec<SelfAssessment>> {
    let rows=sqlx::query("SELECT * FROM learning_assessments WHERE user_id=$1 AND score IS NOT NULL ORDER BY id LIMIT 1001").bind(owner).fetch_all(tx).await.map_err(map_error)?;
    if rows.len() > 1000 {
        return Err(conflict());
    }
    rows.iter().map(rating).collect()
}
impl LearningStore for PostgresStore {
    fn create_model_authorization(
        &self,
        owner: &UserId,
        plan: &str,
        task: &str,
        input: &ModelAuthorizationInput,
    ) -> BoxFuture<'_, StorageResult<ModelAuthorization>> {
        let keys = (id(owner.as_str()), id(plan), id(task));
        let input = input.clone();
        Box::pin(async move {
            model_authorization::create(self, keys.0?, keys.1?, keys.2?, input).await
        })
    }
    fn get_model_authorization(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ModelAuthorization>> {
        let keys = (id(owner.as_str()), id(request));
        Box::pin(async move { model_authorization::get(self, keys.0?, keys.1?).await })
    }
    fn list_model_authorizations(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ModelAuthorizationPage>> {
        let owner = id(owner.as_str());
        let after = after.map(id).transpose();
        Box::pin(async move { model_authorization::list(self, owner?, after?).await })
    }
    fn approve_model_authorization(
        &self,
        owner: &UserId,
        request: &str,
        input: &ModelApproval,
    ) -> BoxFuture<'_, StorageResult<ModelAuthorization>> {
        let keys = (id(owner.as_str()), id(request));
        let input = input.clone();
        Box::pin(async move { model_authorization::approve(self, keys.0?, keys.1?, input).await })
    }
    fn cancel_model_authorization(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ModelAuthorization>> {
        let keys = (id(owner.as_str()), id(request));
        Box::pin(async move { model_authorization::cancel(self, keys.0?, keys.1?).await })
    }
    fn preview_training_model_review(
        &self,
        owner: &UserId,
        plan: &str,
        task: &str,
    ) -> BoxFuture<'_, StorageResult<personal_ai_learning::model_review::ModelReviewPreview>> {
        let keys = (id(owner.as_str()), id(plan), id(task));
        Box::pin(async move { model_review::preview(self, keys.0?, keys.1?, keys.2?).await })
    }

    fn save_training_review(
        &self,
        owner: &UserId,
        plan: &str,
        task: &str,
        input: &personal_ai_storage::learning::review::ReviewInput,
    ) -> BoxFuture<'_, StorageResult<SavedLearningPlan>> {
        let keys = (id(owner.as_str()), id(plan), id(task));
        let input = input.clone();
        Box::pin(async move { reviews::save(self, keys.0?, keys.1?, keys.2?, input).await })
    }
    fn confirm_training_review(
        &self,
        owner: &UserId,
        plan: &str,
        task: &str,
        input: &personal_ai_storage::learning::review::ReviewConfirmation,
    ) -> BoxFuture<'_, StorageResult<SavedLearningPlan>> {
        let keys = (id(owner.as_str()), id(plan), id(task));
        let input = input.clone();
        Box::pin(async move { reviews::confirm(self, keys.0?, keys.1?, keys.2?, input).await })
    }

    fn save_training_evidence(
        &self,
        owner: &UserId,
        plan: &str,
        task: &str,
        request: &str,
        body: &personal_ai_storage::learning::TrainingEvidenceBody,
    ) -> BoxFuture<'_, StorageResult<SavedLearningPlan>> {
        let keys = (id(owner.as_str()), id(plan), id(task), id(request));
        let body = body.clone();
        Box::pin(
            async move { evidence::save(self, keys.0?, keys.1?, keys.2?, keys.3?, body).await },
        )
    }
    fn delete_training_evidence(
        &self,
        owner: &UserId,
        plan: &str,
        task: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<SavedLearningPlan>> {
        let keys = (id(owner.as_str()), id(plan), id(task), id(request));
        Box::pin(async move { evidence::delete(self, keys.0?, keys.1?, keys.2?, keys.3?).await })
    }

    fn learning_progress(
        &self,
        owner: &UserId,
    ) -> BoxFuture<'_, StorageResult<personal_ai_storage::learning::LearningProgress>> {
        let owner = id(owner.as_str());
        Box::pin(async move { progress::read(self, owner?).await })
    }

    fn record_training_result(
        &self,
        owner: &UserId,
        plan: &str,
        task: &str,
        input: &personal_ai_storage::learning::TrainingResultInput,
    ) -> BoxFuture<'_, StorageResult<SavedLearningPlan>> {
        let owner = id(owner.as_str());
        let plan = id(plan);
        let task = id(task);
        let input = input.clone();
        Box::pin(async move { results::record(self, owner?, plan?, task?, input).await })
    }

    fn learning_snapshot(&self, owner: &UserId) -> BoxFuture<'_, StorageResult<LearningSnapshot>> {
        let owner = id(owner.as_str());
        Box::pin(async move {
            let owner = owner?;
            let mut tx = locked(self, owner).await?;
            let saved = LearningSnapshot {
                revision: revision(&mut tx, owner).await?,
                skills: graph(&mut tx, owner).await?,
                assessments: ratings(&mut tx, owner).await?,
            };
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn save_skill(
        &self,
        owner: &UserId,
        skill: &str,
        expected: u64,
        input: &SkillInput,
    ) -> BoxFuture<'_, StorageResult<SkillNode>> {
        let owner = id(owner.as_str());
        let skill = id(skill);
        let input = input.clone();
        Box::pin(async move { skills::save(self, owner?, skill?, expected, input).await })
    }
    fn delete_skill(
        &self,
        owner: &UserId,
        skill: &str,
        expected: u64,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let owner = id(owner.as_str());
        let skill = id(skill);
        Box::pin(async move { skills::delete(self, owner?, skill?, expected).await })
    }
    fn record_assessment(
        &self,
        owner: &UserId,
        request: &str,
        input: &AssessmentInput,
    ) -> BoxFuture<'_, StorageResult<SelfAssessment>> {
        let owner = id(owner.as_str());
        let request = id(request);
        let input = input.clone();
        Box::pin(async move { skills::assess(self, owner?, request?, input).await })
    }
    fn create_learning_plan(
        &self,
        owner: &UserId,
        request: &str,
        input: &LearningPlanInput,
    ) -> BoxFuture<'_, StorageResult<SavedLearningPlan>> {
        let owner = id(owner.as_str());
        let request = id(request);
        let input = input.clone();
        Box::pin(async move { plans::create(self, owner?, request?, input).await })
    }
    fn get_learning_plan(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<SavedLearningPlan>> {
        let owner = id(owner.as_str());
        let request = id(request);
        Box::pin(async move {
            let (owner, request) = (owner?, request?);
            let mut tx = locked(self, owner).await?;
            let saved = plans::read(&mut tx, owner, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn list_learning_plans(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<LearningPlanPage>> {
        let owner = id(owner.as_str());
        let after = after.map(id).transpose();
        Box::pin(async move { plans::list(self, owner?, after?).await })
    }
    fn delete_learning_plan(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let owner = id(owner.as_str());
        let request = id(request);
        Box::pin(async move { plans::delete(self, owner?, request?).await })
    }
}
