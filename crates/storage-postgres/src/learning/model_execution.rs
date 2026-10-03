use super::{
    BoxFuture, PostgresStore, Row, StorageResult, UserId, Uuid, conflict, id, locked, map_error,
    model_authorization, now,
};
use crate::subscription_connections;
use personal_ai_learning::model_review::validate_response;
use personal_ai_storage::{
    learning::model_authorization::{
        ModelAuthorization, ModelReviewClaim, ModelReviewExecutionStore,
    },
    subscription_connections::VerifiedSubscriptionConnection,
};

impl ModelReviewExecutionStore for PostgresStore {
    fn claim_model_review(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Option<ModelReviewClaim>>> {
        let keys = (id(owner.as_str()), id(request));
        Box::pin(async move {
            let (owner, request) = (keys.0?, keys.1?);
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            let saved = model_authorization::read(&mut tx, owner, request, time).await?;
            if saved.status != "authorized" {
                tx.commit().await.map_err(map_error)?;
                return Ok(None);
            }
            let token = Uuid::new_v4();
            let deadline = (time + 90_000)
                .min(i64::try_from(saved.expires_at_unix_ms).map_err(|_| conflict())?);
            let changed = sqlx::query("UPDATE learning_model_authorizations SET status='running',dispatch_token=$3,dispatch_deadline_ms=$4 WHERE user_id=$1 AND request_id=$2 AND dispatch_token IS NULL")
    .bind(owner).bind(request).bind(token).bind(deadline).execute(&mut *tx).await.map_err(map_error)?;
            if changed.rows_affected() != 1 {
                return Err(conflict());
            }
            let authorization = model_authorization::read(&mut tx, owner, request, time).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(Some(ModelReviewClaim {
                owner: UserId::new(owner.to_string()),
                request_id: request.to_string(),
                token: token.to_string(),
                authorization,
            }))
        })
    }
    fn begin_model_review(
        &self,
        claim: &ModelReviewClaim,
        proof: &VerifiedSubscriptionConnection,
    ) -> BoxFuture<'_, StorageResult<bool>> {
        let keys = (
            id(claim.owner.as_str()),
            id(&claim.request_id),
            id(&claim.token),
        );
        let expected = claim.authorization.clone();
        let mut proof = proof.clone();
        Box::pin(async move {
            let (owner, request, token) = (keys.0?, keys.1?, keys.2?);
            let (host, subject) = subscription_connections::validate(&mut proof)?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            let saved = model_authorization::read(&mut tx, owner, request, time).await?;
            let row=sqlx::query("SELECT dispatch_token,sent_ms FROM learning_model_authorizations WHERE user_id=$1 AND request_id=$2").bind(owner).bind(request).fetch_one(&mut *tx).await.map_err(map_error)?;
            if row.get::<Option<Uuid>, _>("dispatch_token") != Some(token) {
                return Err(conflict());
            }
            if saved.status != "running" || row.get::<Option<i64>, _>("sent_ms").is_some() {
                tx.commit().await.map_err(map_error)?;
                return Ok(false);
            }
            if saved != expected {
                return Err(conflict());
            }
            let identity=sqlx::query("SELECT host_id,client_id,subject_hash FROM subscription_connections WHERE user_id=$1 AND id=$2").bind(owner).bind(id(&saved.connection_id)?).fetch_one(&mut *tx).await.map_err(map_error)?;
            if proof.valid_until_unix_ms <= time
                || !proof.models.contains(&saved.model)
                || identity.get::<Uuid, _>("host_id") != host
                || identity.get::<String, _>("client_id") != proof.client_id
                || identity.get::<String, _>("subject_hash") != subject
            {
                return Err(conflict());
            }
            sqlx::query("UPDATE learning_model_authorizations SET sent_ms=$3 WHERE user_id=$1 AND request_id=$2").bind(owner).bind(request).bind(time).execute(&mut *tx).await.map_err(map_error)?;
            sqlx::query("INSERT INTO learning_model_authorization_audit(user_id,request_id,event,at_ms) VALUES($1,$2,'sending',$3)").bind(owner).bind(request).bind(time).execute(&mut *tx).await.map_err(map_error)?;
            tx.commit().await.map_err(map_error)?;
            Ok(true)
        })
    }
    fn finish_model_review(
        &self,
        claim: &ModelReviewClaim,
        output: Option<Vec<u8>>,
    ) -> BoxFuture<'_, StorageResult<ModelAuthorization>> {
        let keys = (
            id(claim.owner.as_str()),
            id(&claim.request_id),
            id(&claim.token),
        );
        Box::pin(async move {
            let (owner, request, token) = (keys.0?, keys.1?, keys.2?);
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            let saved = model_authorization::read(&mut tx, owner, request, time).await?;
            let row=sqlx::query("SELECT dispatch_token,sent_ms FROM learning_model_authorizations WHERE user_id=$1 AND request_id=$2").bind(owner).bind(request).fetch_one(&mut *tx).await.map_err(map_error)?;
            if row.get::<Option<Uuid>, _>("dispatch_token") != Some(token) {
                return Err(conflict());
            }
            if saved.status != "running" {
                tx.commit().await.map_err(map_error)?;
                return Ok(saved);
            }
            let advice = output
                .filter(|_| row.get::<Option<i64>, _>("sent_ms").is_some())
                .and_then(|bytes| {
                    let raw = std::str::from_utf8(&bytes).ok()?;
                    validate_response(saved.preview.as_ref()?, raw).ok()
                })
                .map(serde_json::to_value)
                .transpose()
                .map_err(|_| conflict())?;
            sqlx::query("UPDATE learning_model_authorizations SET status=CASE WHEN $3::jsonb IS NULL THEN 'unknown' ELSE 'succeeded' END,advice=$3 WHERE user_id=$1 AND request_id=$2").bind(owner).bind(request).bind(advice).execute(&mut *tx).await.map_err(map_error)?;
            let saved = model_authorization::read(&mut tx, owner, request, time).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
}
