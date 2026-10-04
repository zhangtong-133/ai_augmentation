use crate::{
    PostgresStore, map_error,
    schedules::{lock_owner, now},
};
use personal_ai_agent_core::answer_authorization::{LIFETIME_MS, intent_digest, prepare};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    answer_authorizations::{
        AnswerApproval, AnswerAuthorization, AnswerAuthorizationStore, AnswerClaim, AnswerMaterial,
        AnswerPreparation, PreparedAnswer, SourceBinding, SourceSelection,
    },
};
use sqlx::{PgConnection, Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;
fn invalid() -> StorageError {
    StorageError::InvalidData("invalid answer authorization".into())
}
fn conflict() -> StorageError {
    StorageError::Conflict("answer authorization unavailable".into())
}
fn key(s: &str) -> StorageResult<Uuid> {
    Uuid::parse_str(s)
        .ok()
        .filter(|v| !v.is_nil() && v.to_string() == s)
        .ok_or_else(invalid)
}
fn record(row: &PgRow) -> AnswerAuthorization {
    AnswerAuthorization {
        request_id: row.get::<Uuid, _>("request_id").to_string(),
        status: row.get("status"),
        digest: row.get("digest"),
        created_at_unix_ms: row.get("created_ms"),
        expires_at_unix_ms: row.get("expires_ms"),
        preview: None,
    }
}
async fn expire(tx: &mut PgConnection, owner: Uuid) -> StorageResult<()> {
    sqlx::query("UPDATE answer_authorizations SET status='expired',question=NULL WHERE user_id=$1 AND status IN ('draft','authorized') AND expires_ms <= floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
        .bind(owner).execute(tx).await.map_err(map_error)?;
    Ok(())
}
async fn terminal(
    tx: &mut PgConnection,
    owner: Uuid,
    item: &mut AnswerAuthorization,
    status: &str,
) -> StorageResult<()> {
    sqlx::query("UPDATE answer_authorizations SET status=$3,question=NULL WHERE user_id=$1 AND request_id=$2")
        .bind(owner).bind(key(&item.request_id)?).bind(status).execute(tx).await.map_err(map_error)?;
    item.status = status.into();
    item.preview = None;
    Ok(())
}
async fn materials(
    tx: &mut PgConnection,
    owner: Uuid,
    sources: &[SourceSelection],
) -> StorageResult<Vec<AnswerMaterial>> {
    let mut result = Vec::new();
    for selection in sources {
        let ordinal = i32::try_from(selection.ordinal.checked_add(1).ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        let row=sqlx::query("SELECT title,source,chunks[$3] AS text FROM documents WHERE user_id=$1 AND id=$2 FOR SHARE")
            .bind(owner).bind(key(&selection.document_id)?).bind(ordinal).fetch_optional(&mut *tx).await.map_err(map_error)?.ok_or(StorageError::NotFound)?;
        result.push(AnswerMaterial {
            selection: selection.clone(),
            title: row.get("title"),
            source: row.get("source"),
            text: row
                .get::<Option<String>, _>("text")
                .ok_or(StorageError::NotFound)?,
        });
    }
    Ok(result)
}
async fn rebuild(tx: &mut PgConnection, owner: Uuid, row: &PgRow) -> StorageResult<PreparedAnswer> {
    let bindings: Vec<SourceBinding> =
        serde_json::from_value(row.get("sources")).map_err(|_| invalid())?;
    let input = AnswerPreparation {
        request_id: row.get::<Uuid, _>("request_id").to_string(),
        question: row
            .get::<Option<String>, _>("question")
            .ok_or_else(invalid)?,
        endpoint: row.get("endpoint"),
        model: row.get("model"),
        sources: bindings.iter().map(|b| b.selection.clone()).collect(),
    };
    let user = UserId::new(owner.to_string());
    if intent_digest(&user, &input)? != row.get::<String, _>("intent_sha256") {
        return Err(invalid());
    }
    let current = materials(tx, owner, &input.sources).await?;
    let (preview, current_bindings, digest) = prepare(
        &user,
        &input,
        current,
        row.get("created_ms"),
        row.get("expires_ms"),
    )?;
    if bindings != current_bindings || digest != row.get::<String, _>("digest") {
        return Err(conflict());
    }
    Ok(preview)
}
async fn read(
    tx: &mut PgConnection,
    owner: Uuid,
    request: Uuid,
) -> StorageResult<AnswerAuthorization> {
    let row = sqlx::query("SELECT * FROM answer_authorizations WHERE user_id=$1 AND request_id=$2")
        .bind(owner)
        .bind(request)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_error)?;
    let mut item = record(&row);
    if !matches!(item.status.as_str(), "draft" | "authorized") {
        return Ok(item);
    }
    if item.expires_at_unix_ms <= now(tx).await? {
        terminal(tx, owner, &mut item, "expired").await?;
        return Ok(item);
    }
    match rebuild(tx, owner, &row).await {
        Ok(preview) => item.preview = Some(preview),
        Err(StorageError::NotFound | StorageError::InvalidData(_) | StorageError::Conflict(_)) => {
            terminal(tx, owner, &mut item, "invalidated").await?;
        }
        Err(e) => return Err(e),
    }
    if item.preview.is_some() && item.expires_at_unix_ms <= now(tx).await? {
        terminal(tx, owner, &mut item, "expired").await?;
    }
    Ok(item)
}
impl PostgresStore {
    async fn answer_tx(&self, owner: Uuid) -> StorageResult<Transaction<'_, Postgres>> {
        let mut tx = self.pool.begin().await.map_err(map_error)?;
        sqlx::query("SET LOCAL statement_timeout='5s'")
            .execute(&mut *tx)
            .await
            .map_err(map_error)?;
        lock_owner(&mut tx, owner).await?;
        Ok(tx)
    }
}
impl AnswerAuthorizationStore for PostgresStore {
    fn prepare_answer(
        &self,
        owner: &UserId,
        input: &AnswerPreparation,
    ) -> BoxFuture<'_, StorageResult<AnswerAuthorization>> {
        let owner = owner.clone();
        let input = input.clone();
        Box::pin(async move {
            let intent = intent_digest(&owner, &input)?;
            let user = key(owner.as_str())?;
            let request = key(&input.request_id)?;
            let mut tx = self.answer_tx(user).await?;
            let existing:Option<String>=sqlx::query_scalar("SELECT intent_sha256 FROM answer_authorizations WHERE user_id=$1 AND request_id=$2").bind(user).bind(request).fetch_optional(&mut *tx).await.map_err(map_error)?;
            if let Some(saved) = existing {
                if saved != intent {
                    return Err(conflict());
                }
                let item = read(&mut tx, user, request).await?;
                tx.commit().await.map_err(map_error)?;
                return Ok(item);
            }
            expire(&mut tx, user).await?;
            let (total,active):(i64,i64)=sqlx::query_as("SELECT count(*),count(*) FILTER(WHERE status IN ('draft','authorized')) FROM answer_authorizations WHERE user_id=$1").bind(user).fetch_one(&mut *tx).await.map_err(map_error)?;
            if total >= 1000 || active >= 20 {
                return Err(conflict());
            }
            let current = materials(&mut tx, user, &input.sources).await?;
            let time = now(&mut tx).await?;
            let (_, bindings, digest) = prepare(&owner, &input, current, time, time + LIFETIME_MS)?;
            sqlx::query("INSERT INTO answer_authorizations(user_id,request_id,endpoint,model,question,sources,intent_sha256,digest,status,created_ms,expires_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'draft',$9,$10)")
                .bind(user).bind(request).bind(&input.endpoint).bind(&input.model).bind(&input.question).bind(serde_json::to_value(bindings).map_err(|_|invalid())?).bind(intent).bind(digest).bind(time).bind(time+LIFETIME_MS).execute(&mut *tx).await.map_err(map_error)?;
            let item = read(&mut tx, user, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(item)
        })
    }
    fn get_answer_authorization(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<AnswerAuthorization>> {
        let keys = key(owner.as_str()).and_then(|u| Ok((u, key(request)?)));
        Box::pin(async move {
            let (u, r) = keys?;
            let mut tx = self.answer_tx(u).await?;
            let item = read(&mut tx, u, r).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(item)
        })
    }
    fn list_answer_authorizations(
        &self,
        owner: &UserId,
    ) -> BoxFuture<'_, StorageResult<Vec<AnswerAuthorization>>> {
        let owner = key(owner.as_str());
        Box::pin(async move {
            let u = owner?;
            let mut tx = self.answer_tx(u).await?;
            expire(&mut tx, u).await?;
            let rows=sqlx::query("SELECT request_id,status,digest,created_ms,expires_ms FROM answer_authorizations WHERE user_id=$1 ORDER BY created_ms DESC,request_id DESC LIMIT 20").bind(u).fetch_all(&mut *tx).await.map_err(map_error)?;
            let result = rows.iter().map(record).collect();
            tx.commit().await.map_err(map_error)?;
            Ok(result)
        })
    }
    fn approve_answer(
        &self,
        owner: &UserId,
        request: &str,
        approval: &AnswerApproval,
    ) -> BoxFuture<'_, StorageResult<AnswerAuthorization>> {
        let keys = key(owner.as_str()).and_then(|u| Ok((u, key(request)?)));
        let approval = approval.clone();
        Box::pin(async move {
            if !approval.acknowledge_sharing || !approval.acknowledge_local_compute {
                return Err(invalid());
            }
            let (u, r) = keys?;
            let mut tx = self.answer_tx(u).await?;
            let mut item = read(&mut tx, u, r).await?;
            if !matches!(item.status.as_str(), "draft" | "authorized")
                || item.digest != approval.digest
            {
                tx.commit().await.map_err(map_error)?;
                return Err(conflict());
            }
            let changed=sqlx::query("UPDATE answer_authorizations SET status='authorized' WHERE user_id=$1 AND request_id=$2 AND expires_ms>floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
                .bind(u).bind(r).execute(&mut *tx).await.map_err(map_error)?.rows_affected();
            if changed == 0 {
                terminal(&mut tx, u, &mut item, "expired").await?;
                tx.commit().await.map_err(map_error)?;
                return Err(conflict());
            }
            item.status = "authorized".into();
            tx.commit().await.map_err(map_error)?;
            Ok(item)
        })
    }
    fn cancel_answer(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<AnswerAuthorization>> {
        let keys = key(owner.as_str()).and_then(|u| Ok((u, key(request)?)));
        Box::pin(async move {
            let (u, r) = keys?;
            let mut tx = self.answer_tx(u).await?;
            let row = sqlx::query("SELECT request_id,status,digest,created_ms,expires_ms FROM answer_authorizations WHERE user_id=$1 AND request_id=$2")
                .bind(u).bind(r).fetch_one(&mut *tx).await.map_err(map_error)?;
            let mut item = record(&row);
            if item.status == "consumed" {
                return Err(conflict());
            }
            if matches!(item.status.as_str(), "draft" | "authorized") {
                terminal(&mut tx, u, &mut item, "cancelled").await?;
            }
            tx.commit().await.map_err(map_error)?;
            Ok(item)
        })
    }
    fn claim_answer(
        &self,
        owner: &UserId,
        request: &str,
        digest: &str,
    ) -> BoxFuture<'_, StorageResult<Option<AnswerClaim>>> {
        let keys = key(owner.as_str()).and_then(|u| Ok((u, key(request)?)));
        let digest = digest.to_owned();
        Box::pin(async move {
            let (u, r) = keys?;
            let mut tx = self.answer_tx(u).await?;
            let mut item = read(&mut tx, u, r).await?;
            if item.status != "authorized" {
                tx.commit().await.map_err(map_error)?;
                return Ok(None);
            }
            if item.digest != digest {
                return Err(conflict());
            }
            let changed=sqlx::query("UPDATE answer_authorizations SET status='consumed',question=NULL WHERE user_id=$1 AND request_id=$2 AND status='authorized' AND expires_ms>floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
                .bind(u).bind(r).execute(&mut *tx).await.map_err(map_error)?.rows_affected();
            if changed == 0 {
                terminal(&mut tx, u, &mut item, "expired").await?;
                tx.commit().await.map_err(map_error)?;
                return Ok(None);
            }
            let preview = item.preview.take().ok_or_else(conflict)?;
            tx.commit().await.map_err(map_error)?;
            Ok(Some(AnswerClaim {
                request_id: item.request_id,
                digest: item.digest,
                preview,
            }))
        })
    }
}
