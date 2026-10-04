//! 独立本地评分，复用单次领取和终态写回，不接受订阅证明。
use crate::{
    PostgresStore, feed_value, feed_value_execution,
    feeds::{conflict, id, locked, now},
    map_error,
};
use personal_ai_domain::UserId;
use personal_ai_llm::local::LocalTarget;
use personal_ai_storage::{
    BoxFuture, StorageResult,
    feed_value::{LocalValueExecutionStore, ValueClaim, ValuePricing, ValueReview},
};
use sqlx::Row;
use uuid::Uuid;

impl LocalValueExecutionStore for PostgresStore {
    fn claim_local_value(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Option<ValueClaim>>> {
        feed_value_execution::claim_value(self, owner, request, true)
    }
    fn finish_local_value(
        &self,
        claim: &ValueClaim,
        output: Option<Vec<u8>>,
    ) -> BoxFuture<'_, StorageResult<ValueReview>> {
        feed_value_execution::finish_value(self, claim, output, true)
    }
    fn begin_local_value(
        &self,
        claim: &ValueClaim,
        target: &LocalTarget,
    ) -> BoxFuture<'_, StorageResult<bool>> {
        let ids = id(claim.owner.as_str())
            .and_then(|owner| Ok((owner, id(&claim.request_id)?, id(&claim.token)?)));
        let expected = claim.review.clone();
        let target = target.clone();
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
            if saved.status != "running"
                || row.get::<Option<i64>, _>("sent_ms").is_some()
                || matches!(&saved.pricing, ValuePricing::Local { profile, .. } if profile != personal_ai_agent_core::feed_value_local::LOCAL_VALUE_PROFILE)
            {
                tx.commit().await.map_err(map_error)?;
                return Ok(false);
            }
            if saved != expected
                || !matches!(&saved.pricing,ValuePricing::Local {endpoint,model,..} if endpoint==target.endpoint() && model==target.model())
                || saved.snapshot.as_ref()
                    != Some(&feed_value::snapshot(&mut tx, owner, saved.created_at_unix_ms).await?)
            {
                return Err(conflict());
            }
            sqlx::query("UPDATE feed_value_reviews SET sent_ms=$3 WHERE user_id=$1 AND id=$2")
                .bind(owner)
                .bind(request)
                .bind(time)
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            sqlx::query("INSERT INTO feed_value_audit(user_id,request_id,event,at_ms) VALUES($1,$2,'sending',$3)").bind(owner).bind(request).bind(time).execute(&mut *tx).await.map_err(map_error)?;
            tx.commit().await.map_err(map_error)?;
            Ok(true)
        })
    }
}
