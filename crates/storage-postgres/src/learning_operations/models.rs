use super::{BTreeMap, LearningQuota, PostgresStore, Row, StorageResult, Uuid, map_error};
use personal_ai_storage::learning_operations::{LearningModelAudit, LearningModelAuditItem};
const SQL: &str = include_str!("models.sql");
pub(super) async fn audit(
    store: &PostgresStore,
    owner: Uuid,
    after: Option<Uuid>,
) -> StorageResult<LearningModelAudit> {
    let mut tx = store.pool.begin().await.map_err(map_error)?;
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
    let time: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
    let summary=sqlx::query(&format!("{SQL} SELECT count(*) AS total,count(*) FILTER(WHERE created_ms >= $2/86400000*86400000) AS today,count(*) FILTER(WHERE cardinality(issues)>0) AS inconsistent FROM audit")).bind(owner).bind(time).fetch_one(&mut *tx).await.map_err(map_error)?;
    let mut counts = BTreeMap::from([
        ("authorizations".into(), summary.get::<i64, _>("total")),
        ("authorizations_today".into(), summary.get("today")),
        (
            "inconsistent_authorizations".into(),
            summary.get("inconsistent"),
        ),
    ]);
    for status in [
        "draft",
        "authorized",
        "running",
        "succeeded",
        "unknown",
        "cancelled",
        "expired",
        "invalidated",
    ] {
        counts.insert(status.into(), 0);
    }
    for row in sqlx::query("SELECT status,count(*) AS total FROM learning_model_authorizations WHERE user_id=$1 GROUP BY status").bind(owner).fetch_all(&mut *tx).await.map_err(map_error)?{counts.insert(row.get("status"),row.get("total"));}
    let diagnostics=sqlx::query(&format!("{SQL} SELECT 'issue' AS kind,unnest(issues) AS code FROM audit UNION SELECT 'warning' AS kind,unnest(warnings) AS code FROM audit ORDER BY kind,code")).bind(owner).bind(time).fetch_all(&mut *tx).await.map_err(map_error)?;
    let mut issues = Vec::new();
    let mut warnings = Vec::new();
    for row in diagnostics {
        if row.get::<String, _>("kind") == "issue" {
            issues.push(row.get::<String, _>("code"));
        } else {
            warnings.push(row.get::<String, _>("code"));
        }
    }
    let rows=sqlx::query(&format!("{SQL} SELECT request_id,plan_id,task_id,connection_id,connection_revision,local,status,created_ms,expires_ms,approved_ms,dispatch_deadline_ms,sent_ms,issues,warnings FROM audit WHERE ($3::uuid IS NULL OR request_id>$3) ORDER BY request_id LIMIT 101")).bind(owner).bind(time).bind(after).fetch_all(&mut *tx).await.map_err(map_error)?;
    let items: Vec<_> = rows.iter().take(100).map(audit_item).collect();
    let next_cursor =
        (rows.len() > 100).then(|| items.last().expect("full audit page").request_id.clone());
    tx.commit().await.map_err(map_error)?;
    let mut quotas = BTreeMap::new();
    for (key, limit) in [("authorizations", 1000), ("authorizations_today", 20)] {
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
    Ok(LearningModelAudit {
        user_id: owner.to_string(),
        snapshot_at_unix_ms: time.to_string(),
        consistent: issues.is_empty(),
        counts,
        quotas,
        issues,
        warnings,
        items,
        next_cursor,
    })
}

fn audit_item(r: &sqlx::postgres::PgRow) -> LearningModelAuditItem {
    LearningModelAuditItem {
        request_id: r.get::<Uuid, _>("request_id").to_string(),
        plan_id: r.get::<Uuid, _>("plan_id").to_string(),
        task_id: r.get::<Uuid, _>("task_id").to_string(),
        execution_kind: if r.get::<bool, _>("local") {
            "local"
        } else {
            "subscription"
        }
        .into(),
        connection_id: r
            .get::<Option<Uuid>, _>("connection_id")
            .map(|v| v.to_string())
            .unwrap_or_default(),
        connection_revision: r
            .get::<Option<i64>, _>("connection_revision")
            .unwrap_or(0)
            .to_string(),
        status: r.get("status"),
        created_at_unix_ms: r.get::<i64, _>("created_ms").to_string(),
        expires_at_unix_ms: r.get::<i64, _>("expires_ms").to_string(),
        approved_at_unix_ms: r
            .get::<Option<i64>, _>("approved_ms")
            .map(|v| v.to_string()),
        dispatch_deadline_unix_ms: r
            .get::<Option<i64>, _>("dispatch_deadline_ms")
            .map(|v| v.to_string()),
        sent_at_unix_ms: r.get::<Option<i64>, _>("sent_ms").map(|v| v.to_string()),
        issues: r.get("issues"),
        warnings: r.get("warnings"),
    }
}
