use crate::{
    PostgresStore, map_error,
    schedules::{lock_owner, now},
};
use personal_ai_domain::UserId;
use personal_ai_feeds::brief::{
    BRIEF_VERSION, BriefCandidate, BriefPlan, normalize_keywords, plan_brief,
};
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    briefs::{BriefPage, BriefPreferences, BriefStatus, BriefStore, BriefSummary, SavedBrief},
};
use sqlx::{PgConnection, Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;
const FIELDS: &str =
    "user_id,request_id,preference_revision,day_start_ms,created_ms,status,digest,plan";
fn invalid() -> StorageError {
    StorageError::InvalidData("invalid brief input".into())
}
fn conflict() -> StorageError {
    StorageError::Conflict("brief preference, quota or snapshot changed".into())
}
fn id(value: &str) -> StorageResult<Uuid> {
    let id = Uuid::parse_str(value).map_err(|_| invalid())?;
    if id.is_nil() {
        return Err(invalid());
    }
    Ok(id)
}
fn number(value: i64) -> StorageResult<u64> {
    u64::try_from(value).map_err(|_| invalid())
}
fn status(value: &str) -> StorageResult<BriefStatus> {
    match value {
        "ready" => Ok(BriefStatus::Ready),
        "invalidated" => Ok(BriefStatus::Invalidated),
        "deleted" => Ok(BriefStatus::Deleted),
        _ => Err(invalid()),
    }
}
async fn locked(store: &PostgresStore, owner: Uuid) -> StorageResult<Transaction<'_, Postgres>> {
    let mut tx = store.pool.begin().await.map_err(map_error)?;
    lock_owner(&mut tx, owner).await?;
    Ok(tx)
}
async fn preferences(tx: &mut PgConnection, owner: Uuid) -> StorageResult<BriefPreferences> {
    let row = sqlx::query("SELECT revision,keywords FROM feed_brief_preferences WHERE user_id=$1")
        .bind(owner)
        .fetch_optional(tx)
        .await
        .map_err(map_error)?;
    let Some(row) = row else {
        return Ok(BriefPreferences {
            revision: 0,
            keywords: Vec::new(),
        });
    };
    let keywords: Vec<String> =
        serde_json::from_value(row.get("keywords")).map_err(|_| invalid())?;
    if normalize_keywords(&keywords).map_err(|_| invalid())? != keywords {
        return Err(invalid());
    }
    Ok(BriefPreferences {
        revision: number(row.get("revision"))?,
        keywords,
    })
}
fn record(row: &PgRow) -> StorageResult<SavedBrief> {
    let owner = row.get::<Uuid, _>("user_id").to_string();
    let saved = SavedBrief {
        request_id: row.get::<Uuid, _>("request_id").to_string(),
        preference_revision: number(row.get("preference_revision"))?,
        day_start_unix_ms: number(row.get("day_start_ms"))?,
        created_at_unix_ms: number(row.get("created_ms"))?,
        status: status(row.get("status"))?,
        digest: row.get("digest"),
        plan: row
            .get::<Option<serde_json::Value>, _>("plan")
            .map(serde_json::from_value::<BriefPlan>)
            .transpose()
            .map_err(|_| invalid())?,
    };
    if let Some(plan) = &saved.plan
        && (plan.version != BRIEF_VERSION
            || plan.user_id != owner
            || plan.request_id != saved.request_id
            || plan.day_start_unix_ms != saved.day_start_unix_ms
            || plan.as_of_unix_ms != saved.created_at_unix_ms
            || plan.digest().map_err(|_| invalid())? != saved.digest)
    {
        return Err(conflict());
    }
    Ok(saved)
}
async fn read(tx: &mut PgConnection, owner: Uuid, request: Uuid) -> StorageResult<SavedBrief> {
    let row = sqlx::query(&format!(
        "SELECT {FIELDS} FROM feed_briefs WHERE user_id=$1 AND request_id=$2"
    ))
    .bind(owner)
    .bind(request)
    .fetch_one(tx)
    .await
    .map_err(map_error)?;
    record(&row)
}
async fn candidates(
    tx: &mut PgConnection,
    owner: Uuid,
    start: i64,
    end: i64,
) -> StorageResult<Vec<BriefCandidate>> {
    let rows=sqlx::query("SELECT e.subscription_id,e.entry_key,e.title,e.summary,e.link,e.published_at,e.first_seen_ms,e.updated_ms,e.last_seen_ms FROM feed_entries e JOIN feed_subscriptions s ON s.user_id=e.user_id AND s.id=e.subscription_id WHERE e.user_id=$1 AND s.enabled AND NOT s.deleted AND e.first_seen_ms>=$2 AND e.first_seen_ms<$3 ORDER BY e.subscription_id,e.entry_key LIMIT 501")
        .bind(owner).bind(start).bind(end).fetch_all(tx).await.map_err(map_error)?;
    if rows.len() > 500 {
        return Err(conflict());
    }
    rows.into_iter()
        .map(|r| {
            Ok(BriefCandidate {
                user_id: owner.to_string(),
                subscription_id: r.get::<Uuid, _>("subscription_id").to_string(),
                entry_key: r.get("entry_key"),
                enabled: true,
                deleted: false,
                title: r.get("title"),
                summary: r.get("summary"),
                link: r.get("link"),
                published_at: r.get("published_at"),
                first_seen_unix_ms: number(r.get("first_seen_ms"))?,
                updated_at_unix_ms: number(r.get("updated_ms"))?,
                last_seen_unix_ms: number(r.get("last_seen_ms"))?,
            })
        })
        .collect()
}
impl BriefStore for PostgresStore {
    fn get_brief_preferences(
        &self,
        owner: &UserId,
    ) -> BoxFuture<'_, StorageResult<BriefPreferences>> {
        let owner = id(owner.as_str());
        Box::pin(async move {
            let owner = owner?;
            let mut tx = locked(self, owner).await?;
            let saved = preferences(&mut tx, owner).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn save_brief_preferences(
        &self,
        owner: &UserId,
        expected_revision: u64,
        keywords: &[String],
    ) -> BoxFuture<'_, StorageResult<BriefPreferences>> {
        let owner = id(owner.as_str());
        let keywords = normalize_keywords(keywords).map_err(|_| invalid());
        Box::pin(async move {
            let (owner, keywords) = (owner?, keywords?);
            let mut tx = locked(self, owner).await?;
            let current = preferences(&mut tx, owner).await?;
            if current.revision != expected_revision {
                return Err(conflict());
            }
            if current.revision > 0 && current.keywords == keywords {
                return Ok(current);
            }
            let revision = i64::try_from(current.revision)
                .map_err(|_| invalid())?
                .checked_add(1)
                .ok_or_else(conflict)?;
            sqlx::query("INSERT INTO feed_brief_preferences(user_id,revision,keywords) VALUES($1,$2,$3) ON CONFLICT(user_id) DO UPDATE SET revision=EXCLUDED.revision,keywords=EXCLUDED.keywords")
                .bind(owner).bind(revision).bind(serde_json::json!(keywords)).execute(&mut *tx).await.map_err(map_error)?;
            tx.commit().await.map_err(map_error)?;
            Ok(BriefPreferences {
                revision: number(revision)?,
                keywords,
            })
        })
    }
    fn create_brief(
        &self,
        owner: &UserId,
        request: &str,
        expected: u64,
    ) -> BoxFuture<'_, StorageResult<SavedBrief>> {
        let owner = id(owner.as_str());
        let request = id(request);
        Box::pin(async move { create(self, owner?, request?, expected).await })
    }
    fn get_brief(&self, owner: &UserId, request: &str) -> BoxFuture<'_, StorageResult<SavedBrief>> {
        let owner = id(owner.as_str());
        let request = id(request);
        Box::pin(async move {
            let mut connection = self.pool.acquire().await.map_err(map_error)?;
            read(&mut connection, owner?, request?).await
        })
    }
    fn list_briefs(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<BriefPage>> {
        let owner = id(owner.as_str());
        let after = after.map(id).transpose();
        Box::pin(async move {
            let rows=sqlx::query("SELECT request_id,day_start_ms,created_ms,status FROM feed_briefs WHERE user_id=$1 AND ($2::uuid IS NULL OR request_id>$2) ORDER BY request_id LIMIT 21")
                .bind(owner?).bind(after?).fetch_all(&self.pool).await.map_err(map_error)?;
            let items: Vec<_> = rows
                .iter()
                .take(20)
                .map(|r| {
                    Ok(BriefSummary {
                        request_id: r.get::<Uuid, _>("request_id").to_string(),
                        day_start_unix_ms: number(r.get("day_start_ms"))?,
                        created_at_unix_ms: number(r.get("created_ms"))?,
                        status: status(r.get("status"))?,
                    })
                })
                .collect::<StorageResult<_>>()?;
            let next_cursor = (rows.len() > 20)
                .then(|| items.last().expect("full brief page").request_id.clone());
            Ok(BriefPage { items, next_cursor })
        })
    }
    fn delete_brief(&self, owner: &UserId, request: &str) -> BoxFuture<'_, StorageResult<()>> {
        let owner = id(owner.as_str());
        let request = id(request);
        Box::pin(async move {
            let (owner, request) = (owner?, request?);
            let mut tx = locked(self, owner).await?;
            let result=sqlx::query("UPDATE feed_briefs SET status='deleted',plan=NULL WHERE user_id=$1 AND request_id=$2")
                .bind(owner).bind(request).execute(&mut *tx).await.map_err(map_error)?;
            if result.rows_affected() == 0 {
                return Err(StorageError::NotFound);
            }
            sqlx::query("DELETE FROM feed_brief_sources WHERE user_id=$1 AND request_id=$2")
                .bind(owner)
                .bind(request)
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            tx.commit().await.map_err(map_error)?;
            Ok(())
        })
    }
}
async fn create(
    store: &PostgresStore,
    owner: Uuid,
    request: Uuid,
    expected: u64,
) -> StorageResult<SavedBrief> {
    let mut tx = locked(store, owner).await?;
    if let Some(row) = sqlx::query(&format!(
        "SELECT {FIELDS} FROM feed_briefs WHERE user_id=$1 AND request_id=$2"
    ))
    .bind(owner)
    .bind(request)
    .fetch_optional(&mut *tx)
    .await
    .map_err(map_error)?
    {
        let saved = record(&row)?;
        if saved.preference_revision != expected {
            return Err(conflict());
        }
        return Ok(saved);
    }
    let pref = preferences(&mut tx, owner).await?;
    if pref.revision != expected {
        return Err(conflict());
    }
    let time = now(&mut tx).await?;
    let start = time / 86_400_000 * 86_400_000;
    let limits=sqlx::query("SELECT count(*) AS total,count(*) FILTER(WHERE day_start_ms=$2) AS daily FROM feed_briefs WHERE user_id=$1")
        .bind(owner).bind(start).fetch_one(&mut *tx).await.map_err(map_error)?;
    if limits.get::<i64, _>("total") >= 1000 || limits.get::<i64, _>("daily") >= 10 {
        return Err(conflict());
    }
    let entries = candidates(
        &mut tx,
        owner,
        start,
        start.checked_add(86_400_000).ok_or_else(invalid)?,
    )
    .await?;
    let plan = plan_brief(
        &UserId::new(owner.to_string()),
        &request.to_string(),
        number(start)?,
        number(time)?,
        &pref.keywords,
        &entries,
    )
    .map_err(|_| conflict())?;
    let digest = plan.digest().map_err(|_| invalid())?;
    sqlx::query("INSERT INTO feed_briefs(user_id,request_id,preference_revision,day_start_ms,created_ms,status,digest,plan) VALUES($1,$2,$3,$4,$5,'ready',$6,$7)")
        .bind(owner).bind(request).bind(i64::try_from(expected).map_err(|_| invalid())?).bind(start).bind(time).bind(digest).bind(serde_json::to_value(plan).map_err(|_| invalid())?).execute(&mut *tx).await.map_err(map_error)?;
    let sources: std::collections::BTreeSet<_> =
        entries.iter().map(|e| e.subscription_id.as_str()).collect();
    for source in sources {
        sqlx::query(
            "INSERT INTO feed_brief_sources(user_id,request_id,subscription_id) VALUES($1,$2,$3)",
        )
        .bind(owner)
        .bind(request)
        .bind(id(source)?)
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;
    }
    let saved = read(&mut tx, owner, request).await?;
    tx.commit().await.map_err(map_error)?;
    Ok(saved)
}
