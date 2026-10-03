//! Durable single-send subscription scoring, scoped by the same owner lock as revocation.
use crate::{
    PostgresStore, feed_value,
    feeds::{conflict, id, locked, now},
    map_error, subscription_connections,
};
use personal_ai_agent_core::feed_value::{decode_value_scores, plan_value_scoring};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageResult,
    feed_value::{FeedValueExecutionStore, ValueClaim, ValuePricing},
    subscription_connections::VerifiedSubscriptionConnection,
};
use sqlx::Row;
use uuid::Uuid;

impl FeedValueExecutionStore for PostgresStore {
    fn claim_subscription_value(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Option<ValueClaim>>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, id(request)?)));
        Box::pin(async move {
            let (owner, request) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            feed_value::expire(&mut tx, owner, time).await?;
            let saved = feed_value::read(&mut tx, owner, request).await?;
            if saved.status != "authorized"
                || !matches!(saved.pricing, ValuePricing::Subscription { .. })
            {
                tx.commit().await.map_err(map_error)?;
                return Ok(None);
            }
            subscription_connections::check_quote(&mut tx, owner, &saved.pricing, time).await?;
            if saved.snapshot.as_ref()
                != Some(&feed_value::snapshot(&mut tx, owner, saved.created_at_unix_ms).await?)
            {
                return Err(conflict());
            }
            let token = Uuid::new_v4();
            let deadline = (time + 90_000).min(saved.expires_at_unix_ms);
            sqlx::query("UPDATE feed_value_reviews SET status='running',dispatch_token=$3,dispatch_deadline_ms=$4 WHERE user_id=$1 AND id=$2")
                .bind(owner).bind(request).bind(token).bind(deadline).execute(&mut *tx).await.map_err(map_error)?;
            let review = feed_value::read(&mut tx, owner, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(Some(ValueClaim {
                owner: UserId::new(owner.to_string()),
                request_id: request.to_string(),
                token: token.to_string(),
                review,
            }))
        })
    }

    fn begin_subscription_value(
        &self,
        claim: &ValueClaim,
        proof: &VerifiedSubscriptionConnection,
    ) -> BoxFuture<'_, StorageResult<bool>> {
        let ids = id(claim.owner.as_str())
            .and_then(|o| Ok((o, id(&claim.request_id)?, id(&claim.token)?)));
        let mut proof = proof.clone();
        let expected = claim.review.clone();
        Box::pin(async move {
            let (owner, request, token) = ids?;
            let (host, subject_hash) = subscription_connections::validate(&mut proof)?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            feed_value::expire(&mut tx, owner, time).await?;
            let saved = feed_value::read(&mut tx, owner, request).await?;
            let row = sqlx::query(
                "SELECT dispatch_token,sent_ms FROM feed_value_reviews WHERE user_id=$1 AND id=$2",
            )
            .bind(owner)
            .bind(request)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
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
            let ValuePricing::Subscription {
                connection_id,
                model,
                ..
            } = &saved.pricing
            else {
                return Err(conflict());
            };
            subscription_connections::check_quote(&mut tx, owner, &saved.pricing, time).await?;
            let identity = sqlx::query("SELECT host_id,client_id,subject_hash FROM subscription_connections WHERE user_id=$1 AND id=$2")
                .bind(owner).bind(id(connection_id)?).fetch_one(&mut *tx).await.map_err(map_error)?;
            if proof.valid_until_unix_ms <= time
                || !proof.models.contains(model)
                || identity.get::<Uuid, _>("host_id") != host
                || identity.get::<String, _>("client_id") != proof.client_id
                || identity.get::<String, _>("subject_hash") != subject_hash
            {
                return Err(conflict());
            }
            // This persisted marker is never reset, including after a crash or lost acknowledgement.
            sqlx::query("UPDATE feed_value_reviews SET sent_ms=$3 WHERE user_id=$1 AND id=$2")
                .bind(owner)
                .bind(request)
                .bind(time)
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            sqlx::query("INSERT INTO feed_value_audit(user_id,request_id,event,at_ms) VALUES($1,$2,'sending',$3)")
                .bind(owner).bind(request).bind(time).execute(&mut *tx).await.map_err(map_error)?;
            tx.commit().await.map_err(map_error)?;
            Ok(true)
        })
    }

    fn finish_subscription_value(
        &self,
        claim: &ValueClaim,
        output: Option<Vec<u8>>,
    ) -> BoxFuture<'_, StorageResult<personal_ai_storage::feed_value::ValueReview>> {
        let ids = id(claim.owner.as_str())
            .and_then(|o| Ok((o, id(&claim.request_id)?, id(&claim.token)?)));
        Box::pin(async move {
            let (owner, request, token) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            feed_value::expire(&mut tx, owner, time).await?;
            let saved = feed_value::read(&mut tx, owner, request).await?;
            let row = sqlx::query(
                "SELECT dispatch_token,sent_ms FROM feed_value_reviews WHERE user_id=$1 AND id=$2",
            )
            .bind(owner)
            .bind(request)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
            if row.get::<Option<Uuid>, _>("dispatch_token") != Some(token) {
                return Err(conflict());
            }
            if saved.status != "running" {
                tx.commit().await.map_err(map_error)?;
                return Ok(saved);
            }
            subscription_connections::check_quote(&mut tx, owner, &saved.pricing, time).await?;
            let scores = output
                .filter(|_| row.get::<Option<i64>, _>("sent_ms").is_some())
                .and_then(|bytes| {
                    let snapshot = saved.snapshot.as_ref()?;
                    let plan = plan_value_scoring(
                        &UserId::new(owner.to_string()),
                        &request.to_string(),
                        snapshot.day_start_unix_ms,
                        snapshot.as_of_unix_ms,
                        &snapshot.keywords,
                        &snapshot.candidates,
                    )
                    .ok()??;
                    decode_value_scores(&plan, &bytes).ok()
                });
            let scores = scores
                .map(serde_json::to_value)
                .transpose()
                .map_err(|_| conflict())?;
            sqlx::query("UPDATE feed_value_reviews SET status=CASE WHEN $3::jsonb IS NULL THEN 'unknown' ELSE 'succeeded' END, scores=$3,snapshot=CASE WHEN $3::jsonb IS NULL THEN NULL ELSE snapshot END WHERE user_id=$1 AND id=$2")
                .bind(owner).bind(request).bind(scores).execute(&mut *tx).await.map_err(map_error)?;
            let saved = feed_value::read(&mut tx, owner, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
}
