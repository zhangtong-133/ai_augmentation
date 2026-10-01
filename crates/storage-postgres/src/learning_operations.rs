use crate::{PostgresStore, map_error};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageResult,
    learning_operations::{
        LearningAudit, LearningAuditItem, LearningOperationsStore, LearningQuota,
    },
};
use sqlx::Row;
use std::collections::BTreeMap;
use uuid::Uuid;
const PLANS: &str = include_str!("learning_operations/plans.sql");
const COUNTS: &str = include_str!("learning_operations/counts.sql");
impl LearningOperationsStore for PostgresStore {
    fn audit_learning(
        &self,
        owner: &UserId,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<LearningAudit>> {
        let owner = super::learning::id(owner.as_str());
        let after = after.map(super::learning::id).transpose();
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
            // Establish the snapshot before sampling the clock; do not lock user writes.
            sqlx::query("SELECT id FROM users WHERE id=$1")
                .bind(owner)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_error)?;
            let time: i64 = sqlx::query_scalar(
                "SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint",
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
            let revision: Option<i64> =
                sqlx::query_scalar("SELECT revision FROM learning_state WHERE user_id=$1")
                    .bind(owner)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(map_error)?;
            let mut counts: BTreeMap<String, i64> = sqlx::query(COUNTS)
                .bind(owner)
                .bind(time)
                .fetch_all(&mut *tx)
                .await
                .map_err(map_error)?
                .iter()
                .map(|r| (r.get("key"), r.get("value")))
                .collect();
            let inconsistent: i64 = sqlx::query_scalar(&format!(
                "{PLANS} SELECT count(*) FROM audit WHERE cardinality(issues)>0"
            ))
            .bind(owner)
            .bind(time)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
            counts.insert("inconsistent_plans".into(), inconsistent);
            let rows = sqlx::query(&format!("{PLANS} SELECT request_id,status,snapshot_revision,created_ms,issues FROM audit WHERE ($3::uuid IS NULL OR request_id>$3) ORDER BY request_id LIMIT 101"))
                .bind(owner).bind(time).bind(after).fetch_all(&mut *tx).await.map_err(map_error)?;
            let items: Vec<_> = rows
                .iter()
                .take(100)
                .map(|r| LearningAuditItem {
                    request_id: r.get::<Uuid, _>("request_id").to_string(),
                    status: r.get("status"),
                    snapshot_revision: r.get::<i64, _>("snapshot_revision").to_string(),
                    created_at_unix_ms: r.get::<i64, _>("created_ms").to_string(),
                    issues: r.get("issues"),
                })
                .collect();
            let next_cursor = (rows.len() > 100)
                .then(|| items.last().expect("full audit page").request_id.clone());
            tx.commit().await.map_err(map_error)?;
            Ok(report(
                owner,
                time,
                revision.unwrap_or(0),
                counts,
                items,
                next_cursor,
            ))
        })
    }
}

fn report(
    owner: Uuid,
    time: i64,
    revision: i64,
    counts: BTreeMap<String, i64>,
    items: Vec<LearningAuditItem>,
    next_cursor: Option<String>,
) -> LearningAudit {
    let mut issues: Vec<String> = [
        "cyclic_skills",
        "skills_with_excess_prerequisites",
        "edges_on_deleted_skills",
        "missing_edge_nodes",
        "invalid_assessment_metadata",
        "revision_below_mutations",
        "inconsistent_plans",
    ]
    .into_iter()
    .filter(|key| counts[*key] > 0)
    .map(str::to_owned)
    .collect();
    let mut quotas = BTreeMap::new();
    let mut warnings = Vec::new();
    for (key, limit) in [
        ("skills", 100),
        ("assessments", 1000),
        ("plans", 1000),
        ("plans_today", 10),
    ] {
        let used = counts[key];
        if used > limit {
            issues.push(format!("{key}_limit_exceeded"));
        }
        if used >= limit {
            warnings.push(format!("{key}_quota_exhausted"));
        }
        quotas.insert(
            key.into(),
            LearningQuota {
                used,
                limit,
                remaining: (limit - used).max(0),
            },
        );
    }
    if counts["skills"] > 100 {
        warnings.push("cycle_check_skipped_over_limit".into());
    }
    LearningAudit {
        user_id: owner.to_string(),
        snapshot_at_unix_ms: time.to_string(),
        revision: revision.to_string(),
        consistent: issues.is_empty(),
        counts,
        quotas,
        issues,
        warnings,
        items,
        next_cursor,
    }
}
