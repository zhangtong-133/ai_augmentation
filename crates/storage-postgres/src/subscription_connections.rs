use crate::{
    PostgresStore,
    feeds::{id, locked, now},
    map_error,
};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    feed_value::ValuePricing,
    feeds::FeedPage,
    subscription_connections::{
        SubscriptionConnection, SubscriptionConnectionStore, VerifiedSubscriptionConnection,
    },
};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row, postgres::PgRow};
use uuid::Uuid;
fn invalid() -> StorageError {
    StorageError::InvalidData("invalid subscription connection".into())
}
fn conflict() -> StorageError {
    StorageError::Conflict("subscription connection changed or unavailable".into())
}
fn decode(row: &PgRow) -> StorageResult<SubscriptionConnection> {
    Ok(SubscriptionConnection {
        id: row.get::<Uuid, _>("id").to_string(),
        label: row.get("label"),
        revision: row.get("revision"),
        status: row.get("status"),
        models: serde_json::from_value(row.get("models")).map_err(|_| invalid())?,
        valid_until_unix_ms: row.get("expires_ms"),
    })
}
pub(super) fn validate(
    input: &mut VerifiedSubscriptionConnection,
) -> StorageResult<(Uuid, String)> {
    let host = input
        .host_id
        .strip_prefix("urn:uuid:")
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(invalid)?;
    if host.get_version_num() != 4
        || host.urn().to_string() != input.host_id
        || !input.client_id.starts_with("oaiapp_")
        || input.client_id.len() > 256
        || !input
            .client_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        || input.subject.is_empty()
        || input.subject.len() > 512
        || input.subject.chars().any(char::is_control)
        || input.label.is_empty()
        || input.label.len() > 64
        || !input
            .label
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        || input.models.is_empty()
        || input.models.len() > 100
        || input
            .models
            .iter()
            .any(|m| m.trim().is_empty() || m.len() > 128 || m.chars().any(char::is_control))
    {
        return Err(invalid());
    }
    input.models.sort();
    input.models.dedup();
    Ok((
        host,
        format!("{:x}", Sha256::digest(input.subject.as_bytes())),
    ))
}
pub(super) async fn expire(tx: &mut PgConnection, owner: Uuid, time: i64) -> StorageResult<()> {
    // Keep the final revision available for expiry/revocation even at the renewal cap.
    sqlx::query("UPDATE subscription_connections SET status='expired',revision=revision+1 WHERE user_id=$1 AND status='active' AND expires_ms<=$2")
        .bind(owner).bind(time).execute(tx).await.map_err(map_error)?;
    Ok(())
}
pub(super) async fn check_quote(
    tx: &mut PgConnection,
    owner: Uuid,
    pricing: &ValuePricing,
    time: i64,
) -> StorageResult<()> {
    let ValuePricing::Subscription {
        provider,
        model,
        configuration_version,
        connection_id,
        valid_until_unix_ms,
    } = pricing
    else {
        return Ok(());
    };
    let row = sqlx::query("SELECT * FROM subscription_connections WHERE user_id=$1 AND id=$2")
        .bind(owner)
        .bind(id(connection_id)?)
        .fetch_optional(tx)
        .await
        .map_err(map_error)?
        .ok_or_else(conflict)?;
    let connection = decode(&row)?;
    if provider != "chatgpt-plan"
        || connection.status != "active"
        || connection.valid_until_unix_ms <= time
        || *valid_until_unix_ms > connection.valid_until_unix_ms
        || *configuration_version != connection.configuration_version()
        || !connection.models.contains(model)
    {
        return Err(conflict());
    }
    Ok(())
}
async fn read(
    tx: &mut PgConnection,
    owner: Uuid,
    connection: Uuid,
) -> StorageResult<SubscriptionConnection> {
    decode(
        &sqlx::query("SELECT * FROM subscription_connections WHERE user_id=$1 AND id=$2")
            .bind(owner)
            .bind(connection)
            .fetch_one(tx)
            .await
            .map_err(map_error)?,
    )
}
impl SubscriptionConnectionStore for PostgresStore {
    fn save_subscription_connection(
        &self,
        owner: &UserId,
        connection: &str,
        expected: i64,
        input: &VerifiedSubscriptionConnection,
    ) -> BoxFuture<'_, StorageResult<SubscriptionConnection>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, id(connection)?)));
        let mut input = input.clone();
        Box::pin(async move {
            let (owner, connection) = ids?;
            let (host, subject) = validate(&mut input)?;
            if !(0..998).contains(&expected) {
                return Err(invalid());
            }
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            if input.valid_until_unix_ms <= time || input.valid_until_unix_ms > time + 3_600_000 {
                return Err(invalid());
            }
            let old =
                sqlx::query("SELECT * FROM subscription_connections WHERE user_id=$1 AND id=$2")
                    .bind(owner)
                    .bind(connection)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(map_error)?;
            if let Some(row) = old {
                let saved = decode(&row)?;
                if row.get::<Uuid, _>("host_id") != host
                    || row.get::<String, _>("client_id") != input.client_id
                    || row.get::<String, _>("subject_hash") != subject
                {
                    return Err(conflict());
                }
                let same = saved.status == "active"
                    && saved.label == input.label
                    && saved.models == input.models
                    && saved.valid_until_unix_ms == input.valid_until_unix_ms;
                if same && (saved.revision == expected || saved.revision == expected + 1) {
                    tx.commit().await.map_err(map_error)?;
                    return Ok(saved);
                }
                if saved.revision != expected {
                    return Err(conflict());
                }
                sqlx::query("UPDATE subscription_connections SET label=$3,models=$4,expires_ms=$5,status='active',revision=revision+1 WHERE user_id=$1 AND id=$2")
                    .bind(owner).bind(connection).bind(&input.label).bind(serde_json::json!(input.models)).bind(input.valid_until_unix_ms).execute(&mut *tx).await.map_err(map_error)?;
            } else {
                if expected != 0 {
                    return Err(conflict());
                }
                let count: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM subscription_connections WHERE user_id=$1",
                )
                .bind(owner)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_error)?;
                if count >= 20 {
                    return Err(conflict());
                }
                sqlx::query("INSERT INTO subscription_connections(user_id,id,host_id,client_id,subject_hash,label,models,revision,status,expires_ms) VALUES($1,$2,$3,$4,$5,$6,$7,1,'active',$8)")
                    .bind(owner).bind(connection).bind(host).bind(&input.client_id).bind(subject).bind(&input.label).bind(serde_json::json!(input.models)).bind(input.valid_until_unix_ms).execute(&mut *tx).await.map_err(|_| conflict())?;
            }
            let saved = read(&mut tx, owner, connection).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn get_subscription_connection(
        &self,
        owner: &UserId,
        connection: &str,
    ) -> BoxFuture<'_, StorageResult<SubscriptionConnection>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, id(connection)?)));
        Box::pin(async move {
            let (owner, connection) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            let saved = read(&mut tx, owner, connection).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
    fn list_subscription_connections(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<FeedPage<SubscriptionConnection>>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, after.map(id).transpose()?)));
        Box::pin(async move {
            let (owner, after) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            let rows=sqlx::query("SELECT * FROM subscription_connections WHERE user_id=$1 AND ($2::uuid IS NULL OR id>$2) ORDER BY id LIMIT 11").bind(owner).bind(after).fetch_all(&mut *tx).await.map_err(map_error)?;
            let mut items = rows.iter().map(decode).collect::<StorageResult<Vec<_>>>()?;
            let next_cursor = if items.len() > 10 {
                items.truncate(10);
                items.last().map(|r| r.id.clone())
            } else {
                None
            };
            tx.commit().await.map_err(map_error)?;
            Ok(FeedPage { items, next_cursor })
        })
    }
    fn revoke_subscription_connection(
        &self,
        owner: &UserId,
        connection: &str,
        expected: i64,
    ) -> BoxFuture<'_, StorageResult<SubscriptionConnection>> {
        let ids = id(owner.as_str()).and_then(|o| Ok((o, id(connection)?)));
        Box::pin(async move {
            let (owner, connection) = ids?;
            let mut tx = locked(self, owner).await?;
            let time = now(&mut tx).await?;
            expire(&mut tx, owner, time).await?;
            let saved = read(&mut tx, owner, connection).await?;
            if !(1..1000).contains(&expected) {
                return Err(invalid());
            }
            if saved.status == "revoked" && saved.revision == expected + 1 {
                tx.commit().await.map_err(map_error)?;
                return Ok(saved);
            }
            if saved.revision != expected {
                return Err(conflict());
            }
            if saved.status != "revoked" {
                sqlx::query("UPDATE subscription_connections SET status='revoked',revision=revision+1 WHERE user_id=$1 AND id=$2").bind(owner).bind(connection).execute(&mut *tx).await.map_err(map_error)?;
            }
            let saved = read(&mut tx, owner, connection).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(saved)
        })
    }
}
