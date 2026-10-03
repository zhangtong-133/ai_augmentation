//! 不可变评分报价和双重同意；尚不写金额账本或派发模型。
use crate::{
    PostgresStore, briefs,
    feeds::{conflict, id, invalid, locked, now},
    map_error,
};
use personal_ai_agent_core::{
    budget::{CostReservation, TokenPrices},
    feed_value::{MAX_OUTPUT_TOKENS, plan_value_scoring},
};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageResult,
    feed_value::{
        FeedValueStore, ValueApproval, ValueAudit, ValuePricing, ValueQuotePlanner, ValueReview,
        ValueSnapshot,
    },
    feeds::FeedPage,
};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row, postgres::PgRow};
use std::sync::Arc;
use uuid::Uuid;

fn text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
fn quote(pricing: &ValuePricing, time: i64) -> StorageResult<(Option<i64>, i64)> {
    match pricing {
        ValuePricing::Api {
            budget: b,
            request_limit,
        } => {
            if [
                &b.provider,
                &b.model,
                &b.configuration_version,
                &b.price_version,
                &b.counter_version,
            ]
            .iter()
            .any(|v| !text(v))
                || b.currency.len() != 3
                || !b.currency.bytes().all(|v| v.is_ascii_uppercase())
                || b.output_token_bound != u64::from(MAX_OUTPUT_TOKENS)
                || b.valid_until_unix_ms <= time
            {
                return Err(invalid());
            }
            let cost = CostReservation::quote(
                TokenPrices {
                    input_per_million: b.input_price_per_million,
                    output_per_million: b.output_price_per_million,
                },
                b.input_token_bound,
                b.output_token_bound,
                *request_limit,
            )
            .map_err(|_| invalid())?;
            Ok((Some(cost.amount()), b.valid_until_unix_ms))
        }
        ValuePricing::Subscription {
            provider,
            model,
            configuration_version,
            connection_id,
            valid_until_unix_ms,
        } => {
            if [provider, model, configuration_version]
                .iter()
                .any(|v| !text(v))
                || id(connection_id)?.to_string() != *connection_id
                || *valid_until_unix_ms <= time
            {
                return Err(invalid());
            }
            Ok((None, *valid_until_unix_ms))
        }
    }
}
fn digest(
    owner: Uuid,
    request: Uuid,
    snapshot: &ValueSnapshot,
    pricing: &ValuePricing,
    expires: i64,
) -> StorageResult<String> {
    let plan = plan_value_scoring(
        &UserId::new(owner.to_string()),
        &request.to_string(),
        snapshot.day_start_unix_ms,
        snapshot.as_of_unix_ms,
        &snapshot.keywords,
        &snapshot.candidates,
    )
    .map_err(|_| invalid())?
    .ok_or_else(invalid)?;
    let bytes = serde_json::to_vec(&(
        "rss-value-review-v1",
        plan.digest(),
        snapshot.preference_revision,
        pricing,
        expires,
    ))
    .map_err(|_| invalid())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn decode(row: &PgRow) -> StorageResult<ValueReview> {
    let saved = ValueReview {
        request_id: row.get::<Uuid, _>("id").to_string(),
        status: row.get("status"),
        digest: row.get("digest"),
        amount: row.get("amount"),
        pricing: serde_json::from_value(row.get("pricing")).map_err(|_| invalid())?,
        created_at_unix_ms: row.get("created_ms"),
        expires_at_unix_ms: row.get("expires_ms"),
        approved_at_unix_ms: row.get("approved_ms"),
        snapshot: row
            .get::<Option<serde_json::Value>, _>("snapshot")
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| invalid())?,
    };
    let (amount, valid_until) = quote(&saved.pricing, saved.created_at_unix_ms)?;
    if amount != saved.amount || saved.expires_at_unix_ms > valid_until {
        return Err(conflict());
    }
    if let Some(snapshot) = &saved.snapshot
        && (i64::try_from(snapshot.as_of_unix_ms).map_err(|_| invalid())?
            != saved.created_at_unix_ms
            || digest(
                row.get("user_id"),
                id(&saved.request_id)?,
                snapshot,
                &saved.pricing,
                saved.expires_at_unix_ms,
            )? != saved.digest)
    {
        return Err(conflict());
    }
    Ok(saved)
}
async fn read(tx: &mut PgConnection, owner: Uuid, request: Uuid) -> StorageResult<ValueReview> {
    let row = sqlx::query("SELECT * FROM feed_value_reviews WHERE user_id=$1 AND id=$2")
        .bind(owner)
        .bind(request)
        .fetch_one(tx)
        .await
        .map_err(map_error)?;
    decode(&row)
}
async fn expire(tx: &mut PgConnection, owner: Uuid, time: i64) -> StorageResult<()> {
    crate::subscription_connections::expire(tx, owner, time).await?;
    sqlx::query("UPDATE feed_value_reviews SET status='expired',snapshot=NULL WHERE user_id=$1 AND status IN ('draft','authorized') AND expires_ms<=$2").bind(owner).bind(time).execute(tx).await.map_err(map_error)?;
    Ok(())
}
async fn snapshot(tx: &mut PgConnection, owner: Uuid, time: i64) -> StorageResult<ValueSnapshot> {
    let preferences = briefs::preferences(tx, owner).await?;
    let start = time / 86_400_000 * 86_400_000;
    Ok(ValueSnapshot {
        day_start_unix_ms: u64::try_from(start).map_err(|_| invalid())?,
        as_of_unix_ms: u64::try_from(time).map_err(|_| invalid())?,
        preference_revision: preferences.revision,
        keywords: preferences.keywords,
        candidates: briefs::candidates(tx, owner, start, start + 86_400_000).await?,
    })
}
impl FeedValueStore for PostgresStore {
    fn preview_feed_value(
        &self,
        owner: &UserId,
        request: &str,
        planner: Arc<dyn ValueQuotePlanner>,
    ) -> BoxFuture<'_, StorageResult<ValueReview>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, id(request)?)));
        Box::pin(async move {
            let (owner, request) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            if let Some(row) =
                sqlx::query("SELECT * FROM feed_value_reviews WHERE user_id=$1 AND id=$2")
                    .bind(owner)
                    .bind(request)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(map_error)?
            {
                let saved = decode(&row)?;
                tx.commit().await.map_err(map_error)?;
                return Ok(saved);
            }
            let counts:(i64,i64)=sqlx::query_as("SELECT count(*),count(*) FILTER (WHERE created_ms >= $2 AND created_ms < $2+86400000) FROM feed_value_reviews WHERE user_id=$1").bind(owner).bind(time/86_400_000*86_400_000).fetch_one(&mut *tx).await.map_err(map_error)?;
            if counts.0 >= 1000 || counts.1 >= 20 {
                return Err(conflict());
            }
            let input = snapshot(&mut tx, owner, time).await?;
            // 先验证候选及空偏好，拒绝调用无意义的计数器。
            plan_value_scoring(
                &UserId::new(owner.to_string()),
                &request.to_string(),
                input.day_start_unix_ms,
                input.as_of_unix_ms,
                &input.keywords,
                &input.candidates,
            )
            .map_err(|_| invalid())?
            .ok_or_else(invalid)?;
            let pricing = planner.quote(
                &UserId::new(owner.to_string()),
                &request.to_string(),
                &input,
            )?;
            let (amount, valid_until) = quote(&pricing, time)?;
            crate::subscription_connections::check_quote(&mut tx, owner, &pricing, time).await?;
            let expires = time
                .checked_add(300_000)
                .ok_or_else(invalid)?
                .min(valid_until);
            let digest = digest(owner, request, &input, &pricing, expires)?;
            sqlx::query("INSERT INTO feed_value_reviews(user_id,id,status,snapshot,pricing,digest,amount,created_ms,expires_ms) VALUES($1,$2,'draft',$3,$4,$5,$6,$7,$8)")
                .bind(owner).bind(request).bind(serde_json::to_value(input).map_err(|_| invalid())?).bind(serde_json::to_value(pricing).map_err(|_| invalid())?).bind(digest).bind(amount).bind(time).bind(expires).execute(&mut *tx).await.map_err(map_error)?;
            let saved = read(&mut tx, owner, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn approve_feed_value(
        &self,
        owner: &UserId,
        request: &str,
        approval: &ValueApproval,
    ) -> BoxFuture<'_, StorageResult<ValueReview>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, id(request)?)));
        let approval = approval.clone();
        Box::pin(async move {
            let (owner, request) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            let saved = read(&mut tx, owner, request).await?;
            let mode_ok = match &saved.pricing {
                ValuePricing::Api { budget, .. } => {
                    approval.currency.as_deref() == Some(budget.currency.as_str())
                        && approval.acknowledge_cost
                        && !approval.acknowledge_subscription_usage
                }
                ValuePricing::Subscription { .. } => {
                    approval.currency.is_none()
                        && !approval.acknowledge_cost
                        && approval.acknowledge_subscription_usage
                }
            };
            if !mode_ok
                || !approval.acknowledge_sharing
                || approval.digest != saved.digest
                || approval.amount != saved.amount
                || !["draft", "authorized"].contains(&saved.status.as_str())
            {
                return Err(conflict());
            }
            crate::subscription_connections::check_quote(&mut tx, owner, &saved.pricing, time)
                .await?;
            let current = snapshot(&mut tx, owner, saved.created_at_unix_ms).await?;
            if saved.snapshot.as_ref() != Some(&current) {
                return Err(conflict());
            }
            if saved.status == "draft" {
                sqlx::query("UPDATE feed_value_reviews SET status='authorized',approved_ms=$3 WHERE user_id=$1 AND id=$2").bind(owner).bind(request).bind(time).execute(&mut *tx).await.map_err(map_error)?;
            }
            let saved = read(&mut tx, owner, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn cancel_feed_value(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ValueReview>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, id(request)?)));
        Box::pin(async move {
            let (owner, request) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            read(&mut tx, owner, request).await?;
            sqlx::query("UPDATE feed_value_reviews SET status='cancelled',snapshot=NULL WHERE user_id=$1 AND id=$2 AND status IN ('draft','authorized')").bind(owner).bind(request).execute(&mut *tx).await.map_err(map_error)?;
            let saved = read(&mut tx, owner, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn get_feed_value(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<ValueReview>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, id(request)?)));
        Box::pin(async move {
            let (owner, request) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            let saved = read(&mut tx, owner, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn list_feed_values(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedPage<ValueReview>>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, after.map(id).transpose()?)));
        Box::pin(async move {
            let (owner, after) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            let rows=sqlx::query("SELECT * FROM feed_value_reviews WHERE user_id=$1 AND ($2::uuid IS NULL OR id>$2) ORDER BY id LIMIT 21").bind(owner).bind(after).fetch_all(&mut *tx).await.map_err(map_error)?;
            let mut items = rows.iter().map(decode).collect::<StorageResult<Vec<_>>>()?;
            let next_cursor = if items.len() > 20 {
                items.truncate(20);
                Some(items[19].request_id.clone())
            } else {
                None
            };
            tx.commit().await.map_err(map_error)?;
            Ok(FeedPage { items, next_cursor })
        })
    }
    fn feed_value_audit(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Vec<ValueAudit>>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, id(request)?)));
        Box::pin(async move {
            let (owner, request) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            read(&mut tx, owner, request).await?;
            let rows=sqlx::query("SELECT event,at_ms FROM feed_value_audit WHERE user_id=$1 AND request_id=$2 ORDER BY sequence").bind(owner).bind(request).fetch_all(&mut *tx).await.map_err(map_error)?;
            let result = rows
                .iter()
                .map(|r| ValueAudit {
                    event: r.get("event"),
                    at_unix_ms: r.get("at_ms"),
                })
                .collect();
            tx.commit().await.map_err(map_error)?;
            Ok(result)
        })
    }
}
