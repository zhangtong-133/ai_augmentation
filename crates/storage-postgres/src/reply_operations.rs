use crate::{PostgresStore, map_error};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    replies::ReplyConfiguration,
    reply_operations::{
        ReplyAuditReceipt, ReplyConfigurationPage, ReplyConfigurationRecord, ReplyLedgerTotals,
        ReplyMoneyAudit, ReplyOperationsStore, ReplySettlementCounts, validate_reply_audit_scope,
        validate_reply_revision,
    },
};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

const CONFIG_FIELDS: &str = "revision,model,budget::text AS budget,valid_until_ms,floor(extract(epoch FROM created_at)*1000)::bigint AS created_ms,floor(extract(epoch FROM disabled_at)*1000)::bigint AS disabled_ms,disabled_at IS NULL AND valid_until_ms>floor(extract(epoch FROM clock_timestamp())*1000)::bigint AS active";

fn configuration(row: &PgRow) -> StorageResult<ReplyConfigurationRecord> {
    Ok(ReplyConfigurationRecord {
        configuration: ReplyConfiguration {
            model: row.get("model"),
            revision: row.get("revision"),
        },
        budget: serde_json::from_str(row.get("budget"))
            .map_err(|_| StorageError::Unavailable("invalid stored reply configuration".into()))?,
        valid_until_unix_ms: row.get("valid_until_ms"),
        created_at_unix_ms: row.get("created_ms"),
        disabled_at_unix_ms: row.get("disabled_ms"),
        active: row.get("active"),
    })
}

fn cursor(after: Option<&str>) -> StorageResult<Option<(Uuid, Uuid)>> {
    after
        .map(|value| {
            let invalid = || StorageError::InvalidData("invalid reply audit cursor".into());
            let (conversation, request) = value.split_once('/').ok_or_else(invalid)?;
            Ok((
                Uuid::parse_str(conversation).map_err(|_| invalid())?,
                Uuid::parse_str(request).map_err(|_| invalid())?,
            ))
        })
        .transpose()
}

fn totals(row: &PgRow) -> ReplyLedgerTotals {
    ReplyLedgerTotals {
        reserved_micro: row.get("reserved_micro"),
        pending_micro: row.get("pending_micro"),
        settled_micro: row.get("settled_micro"),
        retained_micro: row.get("retained_micro"),
        refunded_micro: row.get("refunded_micro"),
        expected_occupied_micro: row.get("expected_micro"),
    }
}

fn counts(row: &PgRow) -> ReplySettlementCounts {
    ReplySettlementCounts {
        requests: row.get("requests"),
        pending: row.get("pending"),
        cancelled_before_dispatch: row.get("cancelled"),
        verified: row.get("verified"),
        retained: row.get("retained"),
        usage_exceeded: row.get("exceeded"),
    }
}

fn receipt(row: &PgRow) -> ReplyAuditReceipt {
    ReplyAuditReceipt {
        conversation_id: row.get::<Uuid, _>("conversation_id").to_string(),
        request_id: row.get::<Uuid, _>("request_id").to_string(),
        model: row.get("model"),
        configuration_revision: row.get("configuration_revision"),
        reserved_micro: row.get("reserved"),
        charged_micro: row.get("charged"),
        settlement: row.get("settlement"),
        created_at_unix_ms: row.get("created_ms"),
        settled_at_unix_ms: row.get("settled_ms"),
        conversation_deleted: row.get("conversation_deleted"),
        reply_status: row.get("reply_status"),
    }
}

impl ReplyOperationsStore for PostgresStore {
    fn list_reply_configurations(
        &self,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ReplyConfigurationPage>> {
        let after = after.map(str::to_owned);
        Box::pin(async move {
            if let Some(after) = &after {
                validate_reply_revision(after)?;
            }
            let rows = sqlx::query(&format!("SELECT {CONFIG_FIELDS} FROM reply_configurations WHERE ($1::text IS NULL OR revision>$1) ORDER BY revision LIMIT 101"))
                .bind(after).fetch_all(&self.pool).await.map_err(map_error)?;
            let more = rows.len() > 100;
            let items: Vec<_> = rows
                .iter()
                .take(100)
                .map(configuration)
                .collect::<StorageResult<_>>()?;
            let next_cursor = more.then(|| {
                items
                    .last()
                    .expect("full configuration page")
                    .configuration
                    .revision
                    .clone()
            });
            Ok(ReplyConfigurationPage { items, next_cursor })
        })
    }

    fn get_reply_configuration(
        &self,
        revision: &str,
    ) -> BoxFuture<'_, StorageResult<ReplyConfigurationRecord>> {
        let revision = revision.to_owned();
        Box::pin(async move {
            validate_reply_revision(&revision)?;
            let row = sqlx::query(&format!(
                "SELECT {CONFIG_FIELDS} FROM reply_configurations WHERE revision=$1"
            ))
            .bind(revision)
            .fetch_one(&self.pool)
            .await
            .map_err(map_error)?;
            configuration(&row)
        })
    }

    fn audit_reply_money(
        &self,
        owner: &UserId,
        day: &str,
        currency: &str,
        after: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ReplyMoneyAudit>> {
        let owner = owner.clone();
        let day = day.to_owned();
        let currency = currency.to_owned();
        let after = cursor(after);
        Box::pin(async move {
            validate_reply_audit_scope(&day, &currency)?;
            let owner_id = Uuid::parse_str(owner.as_str())
                .map_err(|_| StorageError::InvalidData("invalid user id".into()))?;
            let after = after?;
            let mut tx = self.pool.begin().await.map_err(map_error)?;
            // 金额、汇总和当前页必须看见同一个快照；不持有用户/对话锁，不修改或修复数据。
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            sqlx::query("SET LOCAL statement_timeout = '30s'")
                .execute(&mut *tx)
                .await
                .map_err(map_error)?;
            sqlx::query("SELECT id FROM users WHERE id=$1")
                .bind(owner_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_error)?;
            let row = sqlx::query(r"
                WITH totals AS (
                    SELECT count(*) AS requests,
                        count(*) FILTER (WHERE charged IS NULL) AS pending,
                        count(*) FILTER (WHERE settlement='cancelled_before_dispatch') AS cancelled,
                        count(*) FILTER (WHERE settlement='verified') AS verified,
                        count(*) FILTER (WHERE settlement='retained') AS retained,
                        count(*) FILTER (WHERE settlement='usage_exceeded') AS exceeded,
                        COALESCE(sum(reserved),0) AS reserved_micro,
                        COALESCE(sum(reserved) FILTER (WHERE charged IS NULL),0) AS pending_micro,
                        COALESCE(sum(charged),0) AS settled_micro,
                        COALESCE(sum(charged) FILTER (WHERE settlement IN ('retained','usage_exceeded')),0) AS retained_micro,
                        COALESCE(sum(reserved-charged),0) AS refunded_micro,
                        COALESCE(sum(COALESCE(charged,reserved)),0) AS expected_micro
                    FROM reply_money_reservations WHERE user_id=$1 AND day=$2::text::date AND currency=$3
                ), ledger AS (
                    SELECT occupied FROM reply_money_daily WHERE user_id=$1 AND day=$2::text::date AND currency=$3
                )
                SELECT requests,pending,cancelled,verified,retained,exceeded,
                    reserved_micro::text,pending_micro::text,settled_micro::text,retained_micro::text,refunded_micro::text,
                    expected_micro::text,occupied::text AS occupied_micro,
                    (COALESCE(occupied,0)::numeric-expected_micro)::text AS difference_micro,
                    COALESCE(occupied::numeric=expected_micro,requests=0) AS consistent,
                    floor(extract(epoch FROM transaction_timestamp())*1000)::bigint::text AS snapshot_ms
                FROM totals LEFT JOIN ledger ON true
            ")
                .bind(owner_id).bind(&day).bind(&currency).fetch_one(&mut *tx).await.map_err(map_error)?;
            let rows = sqlx::query(r"
                SELECT b.conversation_id,b.request_id,b.model,b.configuration_revision,
                    b.reserved::text,b.charged::text,b.settlement,
                    floor(extract(epoch FROM b.created_at)*1000)::bigint::text AS created_ms,
                    floor(extract(epoch FROM b.settled_at)*1000)::bigint::text AS settled_ms,
                    c.id IS NULL OR c.deleted_at IS NOT NULL AS conversation_deleted,r.status AS reply_status
                FROM reply_money_reservations b
                LEFT JOIN conversations c ON c.id=b.conversation_id AND c.user_id=b.user_id
                LEFT JOIN conversation_replies r ON r.conversation_id=c.id AND r.request_id=b.request_id
                WHERE b.user_id=$1 AND b.day=$2::text::date AND b.currency=$3
                    AND ($4::uuid IS NULL OR (b.conversation_id,b.request_id)>($4,$5::uuid))
                ORDER BY b.conversation_id,b.request_id LIMIT 101
            ")
                .bind(owner_id).bind(&day).bind(&currency)
                .bind(after.map(|a| a.0)).bind(after.map(|a| a.1))
                .fetch_all(&mut *tx).await.map_err(map_error)?;
            let more = rows.len() > 100;
            let items: Vec<_> = rows.iter().take(100).map(receipt).collect();
            let next_cursor = more.then(|| {
                let last = items.last().expect("full audit page");
                format!("{}/{}", last.conversation_id, last.request_id)
            });
            tx.commit().await.map_err(map_error)?;
            Ok(ReplyMoneyAudit {
                user_id: owner.as_str().to_owned(),
                day,
                currency,
                snapshot_at_unix_ms: row.get("snapshot_ms"),
                counts: counts(&row),
                totals: totals(&row),
                ledger_occupied_micro: row.get("occupied_micro"),
                difference_micro: row.get("difference_micro"),
                consistent: row.get("consistent"),
                items,
                next_cursor,
            })
        })
    }
}
