use super::{
    PgConnection, PostgresStore, Row, StorageError, StorageResult, Uuid, conflict, id, integer,
    invalid, locked, map_error, model_review, now, number,
};
use personal_ai_storage::learning::model_authorization::{
    ModelApproval, ModelAuthorization, ModelAuthorizationInput, ModelAuthorizationPage,
};
use sha2::{Digest, Sha256};

fn digest(owner: Uuid, item: &ModelAuthorization, input: &str) -> StorageResult<String> {
    let data = serde_json::to_vec(&(
        "learning-subscription-consent-v1",
        owner.to_string(),
        &item.request_id,
        &item.plan_id,
        &item.task_id,
        &item.connection_id,
        item.connection_revision,
        &item.model,
        input,
        item.created_at_unix_ms,
        item.expires_at_unix_ms,
    ))
    .map_err(|_| invalid())?;
    Ok(format!("{:x}", Sha256::digest(data)))
}
async fn connection(
    tx: &mut PgConnection,
    owner: Uuid,
    key: Uuid,
    revision: u64,
    model: &str,
    time: i64,
) -> StorageResult<i64> {
    let expires: Option<i64> = sqlx::query_scalar("SELECT expires_ms FROM subscription_connections WHERE user_id=$1 AND id=$2 AND revision=$3 AND status='active' AND expires_ms>$4 AND models ? $5")
        .bind(owner).bind(key).bind(integer(revision)?).bind(time).bind(model).fetch_optional(tx).await.map_err(map_error)?;
    expires.ok_or_else(conflict)
}
async fn state(
    tx: &mut PgConnection,
    owner: Uuid,
    request: Uuid,
    status: &str,
) -> StorageResult<()> {
    sqlx::query(
        "UPDATE learning_model_authorizations SET status=$3,advice=NULL WHERE user_id=$1 AND request_id=$2",
    )
    .bind(owner)
    .bind(request)
    .bind(status)
    .execute(tx)
    .await
    .map_err(map_error)?;
    Ok(())
}
pub(super) async fn read(
    tx: &mut PgConnection,
    owner: Uuid,
    request: Uuid,
    time: i64,
) -> StorageResult<ModelAuthorization> {
    let r = sqlx::query(
        "SELECT * FROM learning_model_authorizations WHERE user_id=$1 AND request_id=$2",
    )
    .bind(owner)
    .bind(request)
    .fetch_optional(&mut *tx)
    .await
    .map_err(map_error)?
    .ok_or(StorageError::NotFound)?;
    let mut item = ModelAuthorization {
        request_id: request.to_string(),
        plan_id: r.get::<Uuid, _>("plan_id").to_string(),
        task_id: r.get::<Uuid, _>("task_id").to_string(),
        connection_id: r.get::<Uuid, _>("connection_id").to_string(),
        connection_revision: number(r.get("connection_revision"))?,
        model: r.get("model"),
        status: r.get("status"),
        digest: r.get("digest"),
        created_at_unix_ms: number(r.get("created_ms"))?,
        expires_at_unix_ms: number(r.get("expires_ms"))?,
        approved_at_unix_ms: r
            .get::<Option<i64>, _>("approved_ms")
            .map(number)
            .transpose()?,
        preview: None,
        advice: None,
    };
    if !matches!(
        item.status.as_str(),
        "draft" | "authorized" | "running" | "succeeded"
    ) {
        return Ok(item);
    }
    if item.status == "running"
        && r.get::<Option<i64>, _>("dispatch_deadline_ms")
            .is_none_or(|deadline| deadline <= time)
    {
        item.status = "unknown".into();
    } else if matches!(item.status.as_str(), "draft" | "authorized")
        && integer(item.expires_at_unix_ms)? <= time
    {
        item.status = "expired".into();
    } else {
        let current = async {
            connection(
                tx,
                owner,
                id(&item.connection_id)?,
                item.connection_revision,
                &item.model,
                time,
            )
            .await?;
            let preview =
                model_review::read(tx, owner, id(&item.plan_id)?, id(&item.task_id)?).await?;
            if preview.input().input_digest != r.get::<String, _>("input_digest")
                || digest(owner, &item, &preview.input().input_digest)? != item.digest
            {
                return Err(conflict());
            }
            Ok(preview)
        }
        .await;
        match current {
            Ok(preview) => {
                if item.status == "succeeded" {
                    let advice = r
                        .get::<Option<serde_json::Value>, _>("advice")
                        .and_then(|v| {
                            personal_ai_learning::model_review::validate_response(
                                &preview,
                                &v.to_string(),
                            )
                            .ok()
                        });
                    if advice.is_none() {
                        item.status = "invalidated".into();
                    } else {
                        item.advice = advice;
                        item.preview = Some(preview);
                    }
                } else {
                    item.preview = Some(preview);
                }
            }
            Err(StorageError::Conflict(_) | StorageError::NotFound) => {
                item.status = "invalidated".into();
            }
            Err(e) => return Err(e),
        }
    }
    if item.preview.is_none() {
        state(tx, owner, request, &item.status).await?;
    }
    Ok(item)
}
pub(super) async fn create(
    store: &PostgresStore,
    owner: Uuid,
    plan: Uuid,
    task: Uuid,
    input: ModelAuthorizationInput,
) -> StorageResult<ModelAuthorization> {
    let request = id(&input.request_id)?;
    let key = id(&input.connection_id)?;
    if input.connection_revision == 0
        || input.model.trim().is_empty()
        || input.model.len() > 128
        || input.model.chars().any(char::is_control)
    {
        return Err(invalid());
    }
    let mut tx = locked(store, owner).await?;
    let time = now(&mut tx).await?;
    let exists: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM learning_model_authorizations WHERE user_id=$1 AND request_id=$2)").bind(owner).bind(request).fetch_one(&mut *tx).await.map_err(map_error)?;
    if exists {
        let old = read(&mut tx, owner, request, time).await?;
        if old.plan_id != plan.to_string()
            || old.task_id != task.to_string()
            || old.connection_id != key.to_string()
            || old.connection_revision != input.connection_revision
            || old.model != input.model
        {
            return Err(conflict());
        }
        tx.commit().await.map_err(map_error)?;
        return Ok(old);
    }
    let counts: (i64,i64)=sqlx::query_as("SELECT count(*),count(*) FILTER(WHERE created_ms >= $2) FROM learning_model_authorizations WHERE user_id=$1").bind(owner).bind(time/86_400_000*86_400_000).fetch_one(&mut *tx).await.map_err(map_error)?;
    if counts.0 >= 1000 || counts.1 >= 20 {
        return Err(conflict());
    }
    let expires = connection(
        &mut tx,
        owner,
        key,
        input.connection_revision,
        &input.model,
        time,
    )
    .await?
    .min(time + 300_000);
    let preview = model_review::read(&mut tx, owner, plan, task).await?;
    let mut item = ModelAuthorization {
        request_id: request.to_string(),
        plan_id: plan.to_string(),
        task_id: task.to_string(),
        connection_id: key.to_string(),
        connection_revision: input.connection_revision,
        model: input.model,
        status: "draft".into(),
        digest: String::new(),
        created_at_unix_ms: number(time)?,
        expires_at_unix_ms: number(expires)?,
        approved_at_unix_ms: None,
        preview: None,
        advice: None,
    };
    item.digest = digest(owner, &item, &preview.input().input_digest)?;
    sqlx::query("INSERT INTO learning_model_authorizations(user_id,request_id,plan_id,task_id,connection_id,connection_revision,model,input_digest,digest,status,created_ms,expires_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'draft',$10,$11)")
        .bind(owner).bind(request).bind(plan).bind(task).bind(key).bind(integer(item.connection_revision)?).bind(&item.model).bind(&preview.input().input_digest).bind(&item.digest).bind(time).bind(expires).execute(&mut *tx).await.map_err(map_error)?;
    item.preview = Some(preview);
    tx.commit().await.map_err(map_error)?;
    Ok(item)
}
pub(super) async fn get(
    store: &PostgresStore,
    owner: Uuid,
    request: Uuid,
) -> StorageResult<ModelAuthorization> {
    let mut tx = locked(store, owner).await?;
    let time = now(&mut tx).await?;
    let item = read(&mut tx, owner, request, time).await?;
    tx.commit().await.map_err(map_error)?;
    Ok(item)
}
pub(super) async fn list(
    store: &PostgresStore,
    owner: Uuid,
    after: Option<Uuid>,
) -> StorageResult<ModelAuthorizationPage> {
    let mut tx = locked(store, owner).await?;
    let time = now(&mut tx).await?;
    let keys:Vec<Uuid>=sqlx::query_scalar("SELECT request_id FROM learning_model_authorizations WHERE user_id=$1 AND ($2::uuid IS NULL OR request_id>$2) ORDER BY request_id LIMIT 21").bind(owner).bind(after).fetch_all(&mut *tx).await.map_err(map_error)?;
    let next_cursor = if keys.len() > 20 {
        Some(keys[19].to_string())
    } else {
        None
    };
    let mut items = Vec::new();
    for key in keys.into_iter().take(20) {
        items.push(read(&mut tx, owner, key, time).await?);
    }
    tx.commit().await.map_err(map_error)?;
    Ok(ModelAuthorizationPage { items, next_cursor })
}
pub(super) async fn approve(
    store: &PostgresStore,
    owner: Uuid,
    request: Uuid,
    input: ModelApproval,
) -> StorageResult<ModelAuthorization> {
    if !input.acknowledge_sharing
        || !input.acknowledge_subscription_usage
        || input.digest.len() != 64
        || !input
            .digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    let mut tx = locked(store, owner).await?;
    let time = now(&mut tx).await?;
    let mut item = read(&mut tx, owner, request, time).await?;
    if !matches!(
        item.status.as_str(),
        "draft" | "authorized" | "running" | "succeeded"
    ) || item.digest != input.digest
    {
        tx.commit().await.map_err(map_error)?;
        return Err(conflict());
    }
    if item.status == "draft" {
        sqlx::query("UPDATE learning_model_authorizations SET status='authorized',approved_ms=$3 WHERE user_id=$1 AND request_id=$2").bind(owner).bind(request).bind(time).execute(&mut *tx).await.map_err(map_error)?;
        item.status = "authorized".into();
        item.approved_at_unix_ms = Some(number(time)?);
    }
    tx.commit().await.map_err(map_error)?;
    Ok(item)
}
pub(super) async fn cancel(
    store: &PostgresStore,
    owner: Uuid,
    request: Uuid,
) -> StorageResult<ModelAuthorization> {
    let mut tx = locked(store, owner).await?;
    let time = now(&mut tx).await?;
    let mut item = read(&mut tx, owner, request, time).await?;
    if matches!(
        item.status.as_str(),
        "draft" | "authorized" | "running" | "succeeded"
    ) {
        state(&mut tx, owner, request, "cancelled").await?;
        item.status = "cancelled".into();
        item.preview = None;
        item.advice = None;
    }
    tx.commit().await.map_err(map_error)?;
    Ok(item)
}
