use crate::{PostgresStore, map_error, schedules::uuid};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageResult,
    mcp_credentials::MCP_CREDENTIAL_LIMIT,
    mcp_operations::{McpAudit, McpAuditCounts, McpAuditItem, McpOperationsStore},
};
use sqlx::Row;
use uuid::Uuid;

// Explicit metadata projection: column-only read roles never need host_name or token_digest.
const AUDIT: &str = r"
WITH audit AS (
 SELECT id,created_at,expires_at,revoked_at,
 CASE WHEN revoked_at IS NOT NULL THEN 'revoked'
      WHEN expires_at<=($2::text::timestamptz) THEN 'expired' ELSE 'active' END AS status,
 created_at>($2::text::timestamptz)-INTERVAL '24 hours' AS recent,
 array_remove(ARRAY[
   CASE WHEN scope<>'knowledge_search' THEN 'unsupported_scope' END,
   CASE WHEN created_at>($2::text::timestamptz) THEN 'creation_in_future' END,
   CASE WHEN expires_at<=created_at OR expires_at>created_at+INTERVAL '30 days' THEN 'invalid_validity_window' END,
   CASE WHEN revoked_at<created_at THEN 'revocation_before_creation' END,
   CASE WHEN revoked_at>($2::text::timestamptz) THEN 'revocation_in_future' END
 ],NULL) AS issues
 FROM mcp_credentials WHERE user_id=$1
)
";
impl McpOperationsStore for PostgresStore {
    fn audit_mcp_credentials(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<McpAudit>> {
        let owner = uuid(owner.as_str());
        let after = after.map(uuid).transpose();
        Box::pin(async move {
            let (owner, after) = (owner?, after?);
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            sqlx::query("SET LOCAL statement_timeout = '30s'")
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            sqlx::query("SELECT id FROM users WHERE id=$1")
                .bind(owner)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_error)?;
            // Establish the MVCC snapshot before sampling the clock, so commits
            // visible in that snapshot are not mistaken for future timestamps.
            let clock = sqlx::query(
                "SELECT at::text AS snapshot, floor(extract(epoch FROM at)*1000)::bigint::text AS milliseconds FROM (SELECT clock_timestamp() AS at) clock",
            ).fetch_one(&mut *tx).await.map_err(map_error)?;
            let snapshot: String = clock.get("snapshot");
            let time: String = clock.get("milliseconds");
            let row = sqlx::query(&format!(
                "{AUDIT} SELECT count(*) AS total,
                count(*) FILTER (WHERE status='active') AS active,
                count(*) FILTER (WHERE status='expired') AS expired,
                count(*) FILTER (WHERE status='revoked') AS revoked,
                count(*) FILTER (WHERE recent) AS recent,
                count(*) FILTER (WHERE status='active' OR recent) AS quota,
                count(*) FILTER (WHERE cardinality(issues)>0) AS inconsistent FROM audit"
            ))
            .bind(owner)
            .bind(&snapshot)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
            let counts = McpAuditCounts {
                total: row.get("total"),
                active: row.get("active"),
                expired: row.get("expired"),
                revoked: row.get("revoked"),
                issued_last_24h: row.get("recent"),
                quota_used: row.get("quota"),
                inconsistent: row.get("inconsistent"),
            };
            let rows = sqlx::query(&format!(
                "{AUDIT} SELECT id,status,issues,
                floor(extract(epoch FROM created_at)*1000)::bigint::text AS created,
                floor(extract(epoch FROM expires_at)*1000)::bigint::text AS expires,
                floor(extract(epoch FROM revoked_at)*1000)::bigint::text AS revoked
                FROM audit WHERE ($3::uuid IS NULL OR id>$3) ORDER BY id LIMIT 101"
            ))
            .bind(owner)
            .bind(&snapshot)
            .bind(after)
            .fetch_all(&mut *tx)
            .await
            .map_err(map_error)?;
            let items: Vec<_> = rows
                .iter()
                .take(100)
                .map(|r| McpAuditItem {
                    id: r.get::<Uuid, _>("id").to_string(),
                    status: r.get("status"),
                    created_at_unix_ms: r.get("created"),
                    expires_at_unix_ms: r.get("expires"),
                    revoked_at_unix_ms: r.get("revoked"),
                    issues: r.get("issues"),
                })
                .collect();
            let next_cursor =
                (rows.len() > 100).then(|| items.last().expect("full audit page").id.clone());
            let mut issues = Vec::new();
            if counts.quota_used > MCP_CREDENTIAL_LIMIT {
                issues.push("issuance_quota_exceeded".into());
            }
            if counts.inconsistent > 0 {
                issues.push("invalid_credential_metadata".into());
            }
            tx.commit().await.map_err(map_error)?;
            Ok(McpAudit {
                user_id: owner.to_string(),
                snapshot_at_unix_ms: time,
                quota_limit: MCP_CREDENTIAL_LIMIT,
                remaining_issuance: (MCP_CREDENTIAL_LIMIT - counts.quota_used).max(0),
                consistent: issues.is_empty(),
                counts,
                issues,
                items,
                next_cursor,
            })
        })
    }
}
