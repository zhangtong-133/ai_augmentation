use crate::{PostgresStore, map_error};
use personal_ai_domain::UserId;
use personal_ai_feeds::{
    SubscriptionSnapshot, normalize_source, parser::parse_rss, plan_collection, validate_approval,
};
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    feeds::{
        Collection, CollectionAudit, CollectionClaim, CollectionFailure, CollectionOutcome,
        CollectionStatus, EntryCounts, FeedPage, FeedStore, StoredEntry, Subscription,
        SubscriptionInput,
    },
};
use sqlx::{PgConnection, Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

const SUB_FIELDS: &str = "user_id,id,name,source_url,revision,enabled,deleted";
const COLLECTION_FIELDS: &str = "user_id,request_id,subscription_id,plan,digest,status,created_ms,claimed_ms,deadline_ms,finished_ms,reason,inserted,updated,unchanged";

fn invalid() -> StorageError {
    StorageError::InvalidData("invalid feed input".into())
}
fn conflict() -> StorageError {
    StorageError::Conflict("feed state, quota or consent changed".into())
}
fn id(value: &str) -> StorageResult<Uuid> {
    let id = Uuid::parse_str(value).map_err(|_| invalid())?;
    if id.is_nil() {
        return Err(invalid());
    }
    Ok(id)
}
fn input(mut value: SubscriptionInput) -> StorageResult<SubscriptionInput> {
    value.name = value.name.trim().to_owned();
    if value.name.is_empty()
        || value.name.chars().count() > 120
        || value.name.chars().any(char::is_control)
    {
        return Err(invalid());
    }
    value.source_url = normalize_source(&value.source_url).map_err(|_| invalid())?;
    Ok(value)
}
fn subscription(row: &PgRow) -> StorageResult<Subscription> {
    Ok(Subscription {
        snapshot: SubscriptionSnapshot {
            user_id: row.get::<Uuid, _>("user_id").to_string(),
            subscription_id: row.get::<Uuid, _>("id").to_string(),
            revision: u64::try_from(row.get::<i64, _>("revision")).map_err(|_| invalid())?,
            source_url: row.get("source_url"),
            enabled: row.get("enabled"),
        },
        name: row.get("name"),
        deleted: row.get("deleted"),
    })
}
fn counts(row: &PgRow) -> StorageResult<EntryCounts> {
    Ok(EntryCounts {
        inserted: u32::try_from(row.get::<i32, _>("inserted")).map_err(|_| invalid())?,
        updated: u32::try_from(row.get::<i32, _>("updated")).map_err(|_| invalid())?,
        unchanged: u32::try_from(row.get::<i32, _>("unchanged")).map_err(|_| invalid())?,
    })
}
fn collection(row: &PgRow) -> StorageResult<Collection> {
    let plan: personal_ai_feeds::CollectionPlan =
        serde_json::from_value(row.get("plan")).map_err(|_| invalid())?;
    if plan.user_id != row.get::<Uuid, _>("user_id").to_string()
        || plan.request_id != row.get::<Uuid, _>("request_id").to_string()
        || plan.subscription_id != row.get::<Uuid, _>("subscription_id").to_string()
        || i64::try_from(plan.created_at_unix_ms).map_err(|_| invalid())?
            != row.get::<i64, _>("created_ms")
    {
        return Err(conflict());
    }
    Ok(Collection {
        plan,
        digest: row.get("digest"),
        status: serde_json::from_value(serde_json::Value::String(row.get("status")))
            .map_err(|_| invalid())?,
        claimed_at_unix_ms: row.get("claimed_ms"),
        deadline_unix_ms: row.get("deadline_ms"),
        finished_at_unix_ms: row.get("finished_ms"),
        reason: row.get("reason"),
        counts: counts(row)?,
    })
}
async fn now(tx: &mut PgConnection) -> StorageResult<i64> {
    crate::schedules::now(tx).await
}
async fn locked(store: &PostgresStore, owner: Uuid) -> StorageResult<Transaction<'_, Postgres>> {
    let mut tx = store.pool.begin().await.map_err(map_error)?;
    crate::schedules::lock_owner(&mut tx, owner).await?;
    Ok(tx)
}
async fn read_sub(tx: &mut PgConnection, owner: Uuid, sub: Uuid) -> StorageResult<Subscription> {
    let row = sqlx::query(&format!(
        "SELECT {SUB_FIELDS} FROM feed_subscriptions WHERE user_id=$1 AND id=$2"
    ))
    .bind(owner)
    .bind(sub)
    .fetch_one(tx)
    .await
    .map_err(map_error)?;
    subscription(&row)
}
async fn read_collection(
    tx: &mut PgConnection,
    owner: Uuid,
    request: Uuid,
) -> StorageResult<Collection> {
    let row = sqlx::query(&format!(
        "SELECT {COLLECTION_FIELDS} FROM feed_collections WHERE user_id=$1 AND request_id=$2"
    ))
    .bind(owner)
    .bind(request)
    .fetch_one(tx)
    .await
    .map_err(map_error)?;
    collection(&row)
}
async fn check_active_quota(tx: &mut PgConnection, owner: Uuid) -> StorageResult<()> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM feed_subscriptions WHERE user_id=$1 AND enabled AND NOT deleted",
    )
    .bind(owner)
    .fetch_one(tx)
    .await
    .map_err(map_error)?;
    if count >= 50 {
        return Err(conflict());
    }
    Ok(())
}
async fn audit(tx: &mut PgConnection, owner: Uuid, request: Uuid) -> StorageResult<()> {
    sqlx::query("INSERT INTO feed_collection_audit(user_id,request_id,event,at_ms,reason,inserted,updated,unchanged) SELECT user_id,request_id,status,COALESCE(finished_ms,claimed_ms,created_ms),reason,inserted,updated,unchanged FROM feed_collections WHERE user_id=$1 AND request_id=$2")
        .bind(owner).bind(request).execute(tx).await.map_err(map_error)?;
    Ok(())
}
struct Terminal<'a> {
    status: &'a str,
    reason: Option<&'a str>,
    counts: EntryCounts,
}
async fn terminal(
    tx: &mut PgConnection,
    owner: Uuid,
    request: Uuid,
    end: Terminal<'_>,
) -> StorageResult<Collection> {
    let time = now(tx).await?;
    sqlx::query("UPDATE feed_collections SET status=$3,reason=$4,finished_ms=$5,inserted=$6,updated=$7,unchanged=$8 WHERE user_id=$1 AND request_id=$2")
        .bind(owner).bind(request).bind(end.status).bind(end.reason).bind(time)
        .bind(i32::try_from(end.counts.inserted).map_err(|_| invalid())?)
        .bind(i32::try_from(end.counts.updated).map_err(|_| invalid())?)
        .bind(i32::try_from(end.counts.unchanged).map_err(|_| invalid())?)
        .execute(&mut *tx).await.map_err(map_error)?;
    audit(tx, owner, request).await?;
    read_collection(tx, owner, request).await
}
fn page<T>(mut items: Vec<T>, key: impl Fn(&T) -> String) -> FeedPage<T> {
    let more = items.len() > 20;
    items.truncate(20);
    let next_cursor = if more { items.last().map(key) } else { None };
    FeedPage { items, next_cursor }
}

impl FeedStore for PostgresStore {
    fn create_subscription(
        &self,
        owner: &UserId,
        sub: &str,
        value: &SubscriptionInput,
    ) -> BoxFuture<'_, StorageResult<Subscription>> {
        let ids = id(owner.as_str()).and_then(|owner| Ok((owner, id(sub)?)));
        let value = input(value.clone());
        Box::pin(async move {
            let (owner, sub) = ids?;
            let value = value?;
            let mut tx = locked(self, owner).await?;
            let existing = sqlx::query(&format!(
                "SELECT {SUB_FIELDS} FROM feed_subscriptions WHERE user_id=$1 AND id=$2"
            ))
            .bind(owner)
            .bind(sub)
            .fetch_optional(&mut *tx)
            .await
            .map_err(map_error)?;
            if let Some(row) = existing {
                let saved = subscription(&row)?;
                if saved.deleted
                    || saved.name != value.name
                    || saved.snapshot.source_url != value.source_url
                    || saved.snapshot.enabled != value.enabled
                {
                    return Err(conflict());
                }
                tx.commit().await.map_err(map_error)?;
                return Ok(saved);
            }
            let total: i64 =
                sqlx::query_scalar("SELECT count(*) FROM feed_subscriptions WHERE user_id=$1")
                    .bind(owner)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(map_error)?;
            if total >= 1000 {
                return Err(conflict());
            }
            if value.enabled {
                check_active_quota(&mut tx, owner).await?;
            }
            sqlx::query("INSERT INTO feed_subscriptions(user_id,id,name,source_url,enabled) VALUES($1,$2,$3,$4,$5)")
                .bind(owner).bind(sub).bind(value.name).bind(value.source_url).bind(value.enabled).execute(&mut *tx).await.map_err(map_error)?;
            let saved = read_sub(&mut tx, owner, sub).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn update_subscription(
        &self,
        owner: &UserId,
        sub: &str,
        revision: u64,
        value: &SubscriptionInput,
    ) -> BoxFuture<'_, StorageResult<Subscription>> {
        let ids = id(owner.as_str()).and_then(|owner| Ok((owner, id(sub)?)));
        let value = input(value.clone());
        Box::pin(async move {
            let (owner, sub) = ids?;
            let value = value?;
            let mut tx = locked(self, owner).await?;
            let saved = read_sub(&mut tx, owner, sub).await?;
            if saved.deleted || saved.snapshot.revision != revision {
                return Err(conflict());
            }
            if saved.name == value.name
                && saved.snapshot.source_url == value.source_url
                && saved.snapshot.enabled == value.enabled
            {
                tx.commit().await.map_err(map_error)?;
                return Ok(saved);
            }
            if value.enabled && !saved.snapshot.enabled {
                check_active_quota(&mut tx, owner).await?;
            }
            let revision = i64::try_from(revision)
                .map_err(|_| invalid())?
                .checked_add(1)
                .ok_or_else(conflict)?;
            sqlx::query("UPDATE feed_subscriptions SET name=$3,source_url=$4,enabled=$5,revision=$6 WHERE user_id=$1 AND id=$2")
                .bind(owner).bind(sub).bind(value.name).bind(value.source_url).bind(value.enabled).bind(revision).execute(&mut *tx).await.map_err(map_error)?;
            let saved = read_sub(&mut tx, owner, sub).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn delete_subscription(
        &self,
        owner: &UserId,
        sub: &str,
        revision: u64,
    ) -> BoxFuture<'_, StorageResult<()>> {
        let ids = id(owner.as_str()).and_then(|owner| Ok((owner, id(sub)?)));
        Box::pin(async move {
            let (owner, sub) = ids?;
            let mut tx = locked(self, owner).await?;
            let saved = read_sub(&mut tx, owner, sub).await?;
            if saved.deleted {
                return Ok(());
            }
            if saved.snapshot.revision != revision {
                return Err(conflict());
            }
            let revision = i64::try_from(revision)
                .map_err(|_| invalid())?
                .checked_add(1)
                .ok_or_else(conflict)?;
            sqlx::query("UPDATE feed_subscriptions SET deleted=true,enabled=false,revision=$3,name='deleted',source_url='https://deleted.invalid/' WHERE user_id=$1 AND id=$2")
                .bind(owner).bind(sub).bind(revision).execute(&mut *tx).await.map_err(map_error)?;
            sqlx::query("DELETE FROM feed_entries WHERE user_id=$1 AND subscription_id=$2")
                .bind(owner)
                .bind(sub)
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            tx.commit().await.map_err(map_error)
        })
    }
    fn get_subscription(
        &self,
        owner: &UserId,
        sub: &str,
    ) -> BoxFuture<'_, StorageResult<Subscription>> {
        let ids = id(owner.as_str()).and_then(|owner| Ok((owner, id(sub)?)));
        Box::pin(async move {
            let (owner, sub) = ids?;
            let mut connection = self.pool.acquire().await.map_err(map_error)?;
            let saved = read_sub(&mut connection, owner, sub).await?;
            if saved.deleted {
                return Err(StorageError::NotFound);
            }
            Ok(saved)
        })
    }
    fn list_subscriptions(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedPage<Subscription>>> {
        let owner = id(owner.as_str());
        let after = after.map(id).transpose();
        Box::pin(async move {
            let rows=sqlx::query(&format!("SELECT {SUB_FIELDS} FROM feed_subscriptions WHERE user_id=$1 AND NOT deleted AND ($2::uuid IS NULL OR id>$2) ORDER BY id LIMIT 21"))
                .bind(owner?).bind(after?).fetch_all(&self.pool).await.map_err(map_error)?;
            Ok(page(
                rows.iter()
                    .map(subscription)
                    .collect::<StorageResult<Vec<_>>>()?,
                |s| s.snapshot.subscription_id.clone(),
            ))
        })
    }
    fn preview_collection(
        &self,
        owner: &UserId,
        sub: &str,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Collection>> {
        let ids = id(owner.as_str()).and_then(|owner| Ok((owner, id(sub)?, id(request)?)));
        Box::pin(async move {
            let (owner, sub, request) = ids?;
            let mut tx = locked(self, owner).await?;
            let existing=sqlx::query(&format!("SELECT {COLLECTION_FIELDS} FROM feed_collections WHERE user_id=$1 AND request_id=$2"))
                .bind(owner).bind(request).fetch_optional(&mut *tx).await.map_err(map_error)?;
            if let Some(row) = existing {
                let saved = collection(&row)?;
                if saved.plan.subscription_id != sub.to_string() {
                    return Err(conflict());
                }
                tx.commit().await.map_err(map_error)?;
                return Ok(saved);
            }
            let current = read_sub(&mut tx, owner, sub).await?;
            let time = now(&mut tx).await?;
            let plan = plan_collection(
                &UserId::new(owner.to_string()),
                &current.snapshot,
                &request.to_string(),
                u64::try_from(time).map_err(|_| invalid())?,
            )
            .map_err(|_| conflict())?;
            let count:i64=sqlx::query_scalar("SELECT count(*) FROM feed_collections WHERE user_id=$1 AND created_ms>=$2-86400000")
                .bind(owner).bind(time).fetch_one(&mut *tx).await.map_err(map_error)?;
            if count >= 100 {
                return Err(conflict());
            }
            let digest = plan.consent_digest().map_err(|_| invalid())?;
            sqlx::query("INSERT INTO feed_collections(user_id,request_id,subscription_id,plan,digest,created_ms) VALUES($1,$2,$3,$4,$5,$6)")
                .bind(owner).bind(request).bind(sub).bind(serde_json::to_value(plan).map_err(|_| invalid())?).bind(digest).bind(time).execute(&mut *tx).await.map_err(map_error)?;
            audit(&mut tx, owner, request).await?;
            let saved = read_collection(&mut tx, owner, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn get_collection(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Collection>> {
        let ids = id(owner.as_str()).and_then(|owner| Ok((owner, id(request)?)));
        Box::pin(async move {
            let (owner, request) = ids?;
            let mut connection = self.pool.acquire().await.map_err(map_error)?;
            read_collection(&mut connection, owner, request).await
        })
    }
    fn list_collections(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedPage<Collection>>> {
        let owner = id(owner.as_str());
        let after = after.map(id).transpose();
        Box::pin(async move {
            let rows=sqlx::query(&format!("SELECT {COLLECTION_FIELDS} FROM feed_collections WHERE user_id=$1 AND ($2::uuid IS NULL OR request_id>$2) ORDER BY request_id LIMIT 21"))
                .bind(owner?).bind(after?).fetch_all(&self.pool).await.map_err(map_error)?;
            Ok(page(
                rows.iter()
                    .map(collection)
                    .collect::<StorageResult<Vec<_>>>()?,
                |c| c.plan.request_id.clone(),
            ))
        })
    }
    fn claim_collection(
        &self,
        owner: &UserId,
        request: &str,
        accepted: &str,
    ) -> BoxFuture<'_, StorageResult<CollectionClaim>> {
        let ids = id(owner.as_str()).and_then(|owner| Ok((owner, id(request)?)));
        let accepted = accepted.to_owned();
        Box::pin(async move {
            let (owner, request) = ids?;
            let mut tx = locked(self, owner).await?;
            let saved = read_collection(&mut tx, owner, request).await?;
            if saved.status != CollectionStatus::Draft || saved.digest != accepted {
                return Err(conflict());
            }
            let current = read_sub(&mut tx, owner, id(&saved.plan.subscription_id)?).await?;
            let time = now(&mut tx).await?;
            let user = UserId::new(owner.to_string());
            validate_approval(
                &user,
                &current.snapshot,
                &saved.plan,
                &accepted,
                u64::try_from(time).map_err(|_| invalid())?,
            )
            .map_err(|_| conflict())?;
            let quota=sqlx::query("SELECT count(*) FILTER (WHERE claimed_ms>=$2 AND claimed_ms<$2+86400000) AS daily,count(*) FILTER (WHERE status='running') AS running FROM feed_collections WHERE user_id=$1 AND claimed_ms IS NOT NULL")
                .bind(owner).bind(time/86_400_000*86_400_000).fetch_one(&mut *tx).await.map_err(map_error)?;
            if quota.get::<i64, _>("daily") >= 20 || quota.get::<i64, _>("running") > 0 {
                return Err(conflict());
            }
            let claim = Uuid::new_v4();
            sqlx::query("UPDATE feed_collections SET status='running',accepted_digest=$3,claimed_ms=$4,deadline_ms=$4+60000,claim_id=$5 WHERE user_id=$1 AND request_id=$2")
                .bind(owner).bind(request).bind(accepted).bind(time).bind(claim).execute(&mut *tx).await.map_err(map_error)?;
            audit(&mut tx, owner, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(CollectionClaim {
                owner: user,
                request_id: request.to_string(),
                claim_id: claim.to_string(),
                plan: saved.plan,
            })
        })
    }
    fn cancel_collection(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Collection>> {
        let ids = id(owner.as_str()).and_then(|owner| Ok((owner, id(request)?)));
        Box::pin(async move {
            let (owner, request) = ids?;
            let mut tx = locked(self, owner).await?;
            let saved = read_collection(&mut tx, owner, request).await?;
            if saved.status == CollectionStatus::Cancelled {
                return Ok(saved);
            }
            if saved.status != CollectionStatus::Draft {
                return Err(conflict());
            }
            let saved = terminal(
                &mut tx,
                owner,
                request,
                Terminal {
                    status: "cancelled",
                    reason: None,
                    counts: EntryCounts::default(),
                },
            )
            .await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn finish_collection(
        &self,
        claim: &CollectionClaim,
        outcome: CollectionOutcome,
    ) -> BoxFuture<'_, StorageResult<Collection>> {
        let claim = claim.clone();
        Box::pin(async move { self.finish_feed(claim, outcome).await })
    }
    fn recover_collection(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Collection>> {
        let ids = id(owner.as_str()).and_then(|owner| Ok((owner, id(request)?)));
        Box::pin(async move {
            let (owner, request) = ids?;
            let mut tx = locked(self, owner).await?;
            let saved = read_collection(&mut tx, owner, request).await?;
            if saved.status != CollectionStatus::Running {
                return Ok(saved);
            }
            if saved.deadline_unix_ms.ok_or_else(conflict)? > now(&mut tx).await? {
                return Err(conflict());
            }
            let saved = terminal(
                &mut tx,
                owner,
                request,
                Terminal {
                    status: "unknown",
                    reason: Some("execution_expired"),
                    counts: EntryCounts::default(),
                },
            )
            .await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn list_feed_entries(
        &self,
        owner: &UserId,
        sub: &str,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedPage<StoredEntry>>> {
        let ids = id(owner.as_str()).and_then(|owner| Ok((owner, id(sub)?)));
        let after = after.map(str::to_owned);
        Box::pin(async move {
            let (owner, sub) = ids?;
            if after.as_ref().is_some_and(|s| s.len() > 69) {
                return Err(invalid());
            }
            // 单条查询包含订阅删除边界，不在检查和读取之间留下竞态。
            let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM feed_subscriptions WHERE user_id=$1 AND id=$2 AND NOT deleted)")
                .bind(owner).bind(sub).fetch_one(&self.pool).await.map_err(map_error)?;
            if !exists {
                return Err(StorageError::NotFound);
            }
            let rows=sqlx::query("SELECT e.* FROM feed_entries e JOIN feed_subscriptions s ON s.user_id=e.user_id AND s.id=e.subscription_id WHERE e.user_id=$1 AND e.subscription_id=$2 AND NOT s.deleted AND ($3::text IS NULL OR entry_key>$3) ORDER BY entry_key LIMIT 21")
                .bind(owner).bind(sub).bind(after).fetch_all(&self.pool).await.map_err(map_error)?;
            let entries = rows
                .iter()
                .map(|r| StoredEntry {
                    entry_key: r.get("entry_key"),
                    title: r.get("title"),
                    summary: r.get("summary"),
                    link: r.get("link"),
                    published_at: r.get("published_at"),
                    content_digest: r.get("content_digest"),
                    first_seen_unix_ms: r.get("first_seen_ms"),
                    updated_at_unix_ms: r.get("updated_ms"),
                    last_seen_unix_ms: r.get("last_seen_ms"),
                })
                .collect();
            Ok(page(entries, |e| e.entry_key.clone()))
        })
    }
    fn collection_audit(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<Vec<CollectionAudit>>> {
        let ids = id(owner.as_str()).and_then(|owner| Ok((owner, id(request)?)));
        Box::pin(async move {
            let (owner, request) = ids?;
            let mut connection = self.pool.acquire().await.map_err(map_error)?;
            read_collection(&mut connection, owner, request).await?;
            let rows=sqlx::query("SELECT event,at_ms,reason,inserted,updated,unchanged FROM feed_collection_audit WHERE user_id=$1 AND request_id=$2 ORDER BY at_ms,CASE event WHEN 'draft' THEN 0 WHEN 'running' THEN 1 ELSE 2 END LIMIT 3")
                .bind(owner).bind(request).fetch_all(&mut *connection).await.map_err(map_error)?;
            rows.iter()
                .map(|r| {
                    Ok(CollectionAudit {
                        event: r.get("event"),
                        at_unix_ms: r.get("at_ms"),
                        reason: r.get("reason"),
                        counts: counts(r)?,
                    })
                })
                .collect()
        })
    }
}

impl PostgresStore {
    async fn finish_feed(
        &self,
        claim: CollectionClaim,
        outcome: CollectionOutcome,
    ) -> StorageResult<Collection> {
        // 最多 1 MiB，事务外解析；结果只在重新核对凭据和订阅后写入。
        let parsed = match &outcome {
            CollectionOutcome::Response(bytes) => Some(parse_rss(bytes)),
            CollectionOutcome::Failure(_) => None,
        };
        let (owner, request, claim_id) = (
            id(claim.owner.as_str())?,
            id(&claim.request_id)?,
            id(&claim.claim_id)?,
        );
        let mut tx = locked(self, owner).await?;
        let saved = read_collection(&mut tx, owner, request).await?;
        let stored_claim: Option<Uuid> = sqlx::query_scalar(
            "SELECT claim_id FROM feed_collections WHERE user_id=$1 AND request_id=$2",
        )
        .bind(owner)
        .bind(request)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_error)?;
        if stored_claim != Some(claim_id) || claim.plan != saved.plan {
            return Err(conflict());
        }
        if saved.status != CollectionStatus::Running {
            if matches!(
                saved.status,
                CollectionStatus::Succeeded | CollectionStatus::Failed | CollectionStatus::Unknown
            ) {
                return Ok(saved);
            }
            return Err(conflict());
        }
        let current = read_sub(&mut tx, owner, id(&saved.plan.subscription_id)?).await?;
        let time = now(&mut tx).await?;
        let mut end = Terminal {
            status: "succeeded",
            reason: None,
            counts: EntryCounts::default(),
        };
        if saved.deadline_unix_ms.ok_or_else(conflict)? <= time {
            end.status = "unknown";
            end.reason = Some("execution_expired");
        } else if current.deleted
            || !current.snapshot.enabled
            || current.snapshot.revision != saved.plan.subscription_revision
            || current.snapshot.source_url != saved.plan.source_url
        {
            end.status = "failed";
            end.reason = Some("subscription_changed");
        } else {
            match parsed {
                Some(Ok(feed)) => {
                    let sub = id(&saved.plan.subscription_id)?;
                    for entry in feed.entries {
                        let digest = entry.content_digest();
                        let old:Option<String>=sqlx::query_scalar("SELECT content_digest FROM feed_entries WHERE user_id=$1 AND subscription_id=$2 AND entry_key=$3")
                            .bind(owner).bind(sub).bind(entry.entry_key()).fetch_optional(&mut *tx).await.map_err(map_error)?;
                        match old.as_deref() {
                            None => end.counts.inserted += 1,
                            Some(value) if value == digest => end.counts.unchanged += 1,
                            Some(_) => end.counts.updated += 1,
                        }
                        sqlx::query("INSERT INTO feed_entries(user_id,subscription_id,entry_key,title,summary,link,published_at,content_digest,first_seen_ms,updated_ms,last_seen_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$9,$9) ON CONFLICT(user_id,subscription_id,entry_key) DO UPDATE SET title=EXCLUDED.title,summary=EXCLUDED.summary,link=EXCLUDED.link,published_at=EXCLUDED.published_at,content_digest=EXCLUDED.content_digest,last_seen_ms=EXCLUDED.last_seen_ms,updated_ms=CASE WHEN feed_entries.content_digest<>EXCLUDED.content_digest THEN EXCLUDED.updated_ms ELSE feed_entries.updated_ms END")
                            .bind(owner).bind(sub).bind(entry.entry_key()).bind(&entry.title).bind(&entry.summary).bind(&entry.link).bind(&entry.published_at).bind(digest).bind(time).execute(&mut *tx).await.map_err(map_error)?;
                    }
                }
                Some(Err(_)) => {
                    end.status = "failed";
                    end.reason = Some("parse");
                }
                None => {
                    if matches!(
                        outcome,
                        CollectionOutcome::Failure(CollectionFailure::Unknown)
                    ) {
                        end.status = "unknown";
                        end.reason = Some("unknown");
                    } else {
                        end.status = "failed";
                        end.reason = Some("transport");
                    }
                }
            }
        }
        let saved = terminal(&mut tx, owner, request, end).await?;
        tx.commit().await.map_err(map_error)?;
        Ok(saved)
    }
}
