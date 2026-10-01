use crate::{PostgresStore, map_error};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    StorageError, StorageResult,
    mcp_credentials::{McpCredential, NewMcpCredential},
};
use sqlx::Row;
use uuid::Uuid;

fn owner_uuid(owner: &UserId) -> StorageResult<Uuid> {
    Uuid::parse_str(owner.as_str()).map_err(|_| StorageError::InvalidData("invalid owner".into()))
}
fn credential(row: &sqlx::postgres::PgRow) -> McpCredential {
    McpCredential {
        id: row.get::<Uuid, _>("id").to_string(),
        host_name: row.get("host_name"),
        scope: row.get("scope"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
        revoked_at: row.get("revoked_at"),
    }
}
impl PostgresStore {
    pub(crate) async fn create_mcp(
        &self,
        owner: &UserId,
        input: &NewMcpCredential,
    ) -> StorageResult<McpCredential> {
        if !input.valid() {
            return Err(StorageError::InvalidData("invalid MCP credential".into()));
        }
        let owner = owner_uuid(owner)?;
        let mut tx = self.pool.begin().await.map_err(map_error)?;
        // Serialize issuance and password resets for this owner.
        sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
            .bind(owner)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
        // Recheck the issuing session after the owner lock: a concurrent password
        // reset must not allow a previously authenticated request to mint a new token.
        sqlx::query("SELECT token_digest FROM sessions WHERE user_id=$1 AND token_digest=$2 AND expires_at>clock_timestamp() FOR SHARE")
            .bind(owner).bind(&input.session_digest).fetch_one(&mut *tx).await.map_err(map_error)?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_credentials WHERE user_id=$1 AND ((revoked_at IS NULL AND expires_at>NOW()) OR created_at>NOW()-INTERVAL '24 hours')")
            .bind(owner).fetch_one(&mut *tx).await.map_err(map_error)?;
        if count >= 20 {
            return Err(StorageError::Conflict(
                "MCP credential quota reached".into(),
            ));
        }
        let row = sqlx::query("INSERT INTO mcp_credentials(id,user_id,token_digest,host_name,expires_at) VALUES($1,$2,$3,$4,NOW()+make_interval(days => $5)) RETURNING id,host_name,scope,created_at::text,expires_at::text,revoked_at::text")
            .bind(Uuid::new_v4()).bind(owner).bind(&input.digest).bind(input.host_name.trim()).bind(input.days)
            .fetch_one(&mut *tx).await.map_err(map_error)?;
        tx.commit().await.map_err(map_error)?;
        Ok(credential(&row))
    }
    pub(crate) async fn list_mcp(&self, owner: &UserId) -> StorageResult<Vec<McpCredential>> {
        let rows = sqlx::query("SELECT id,host_name,scope,created_at::text,expires_at::text,revoked_at::text FROM mcp_credentials WHERE user_id=$1 ORDER BY (revoked_at IS NULL AND expires_at>NOW()) DESC,created_at DESC,id DESC LIMIT 100")
            .bind(owner_uuid(owner)?).fetch_all(&self.pool).await.map_err(map_error)?;
        Ok(rows.iter().map(credential).collect())
    }
    pub(crate) async fn revoke_mcp(&self, owner: &UserId, id: &str) -> StorageResult<()> {
        let id = Uuid::parse_str(id)
            .map_err(|_| StorageError::InvalidData("invalid credential id".into()))?;
        let result = sqlx::query("UPDATE mcp_credentials SET revoked_at=COALESCE(revoked_at,NOW()) WHERE user_id=$1 AND id=$2")
            .bind(owner_uuid(owner)?).bind(id).execute(&self.pool).await.map_err(map_error)?;
        if result.rows_affected() == 0 {
            return Err(StorageError::NotFound);
        }
        Ok(())
    }
    pub(crate) async fn mcp_owner(&self, digest: &str) -> StorageResult<UserId> {
        let id: Uuid = sqlx::query_scalar("SELECT user_id FROM mcp_credentials WHERE token_digest=$1 AND scope='knowledge_search' AND revoked_at IS NULL AND expires_at>clock_timestamp()")
            .bind(digest).fetch_one(&self.pool).await.map_err(map_error)?;
        Ok(UserId::new(id.to_string()))
    }
}
