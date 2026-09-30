use super::*;
use personal_ai_storage::reply_operations::ReplyOperationsStore;

async fn day(pool: &sqlx::PgPool, request: &str) -> String {
    sqlx::query_scalar("SELECT day::text FROM reply_money_reservations WHERE request_id=$1")
        .bind(Uuid::parse_str(request).unwrap())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)] // 同一账本覆盖全部结算类型、归属和删除后的审计。
async fn audit_includes_all_settlements_and_deleted_conversations_without_crossing_scope() {
    let (store, pool, owner, conversation) = fixture().await;
    let requests: Vec<_> = (0..5).map(|_| id()).collect();
    reserve(&store, &owner, &conversation, &requests[1], 10000)
        .await
        .unwrap();
    store
        .cancel_reply(&owner, &conversation, &requests[1])
        .await
        .unwrap();
    for (index, outcome, usage) in [
        (
            2,
            ReplyOutcome::Succeeded("回答".into()),
            Some(ReplyUsage {
                input_tokens: 80,
                output_tokens: 20,
            }),
        ),
        (3, ReplyOutcome::Unknown, None),
        (
            4,
            ReplyOutcome::Succeeded("回答".into()),
            Some(ReplyUsage {
                input_tokens: 99999,
                output_tokens: 20,
            }),
        ),
    ] {
        reserve(&store, &owner, &conversation, &requests[index], 10000)
            .await
            .unwrap();
        store
            .claim_reply(&owner, &conversation, &requests[index])
            .await
            .unwrap();
        store
            .finish_budgeted_reply(&owner, &conversation, &requests[index], outcome, usage)
            .await
            .unwrap();
    }
    reserve(&store, &owner, &conversation, &requests[0], 10000)
        .await
        .unwrap();
    let audit_day = day(&pool, &requests[0]).await;
    let (_, foreign_pool, foreign, foreign_conversation) = fixture().await;
    reserve(&store, &foreign, &foreign_conversation, &id(), 10000)
        .await
        .unwrap();
    let report = store
        .audit_reply_money(&owner, &audit_day, "USD", None)
        .await
        .unwrap();
    assert!(report.consistent);
    assert_eq!(report.counts.requests, 5);
    assert_eq!(report.counts.pending, 1);
    assert_eq!(report.counts.cancelled_before_dispatch, 1);
    assert_eq!(report.counts.verified, 1);
    assert_eq!(report.counts.retained, 1);
    assert_eq!(report.counts.usage_exceeded, 1);
    assert_eq!(report.totals.reserved_micro, "5620");
    assert_eq!(report.totals.pending_micro, "1124");
    assert_eq!(report.totals.settled_micro, "2348");
    assert_eq!(report.totals.refunded_micro, "2148");
    assert_eq!(report.totals.retained_micro, "2248");
    assert_eq!(report.ledger_occupied_micro.as_deref(), Some("3472"));
    assert_eq!(report.difference_micro, "0");
    assert!(
        report
            .items
            .iter()
            .all(|r| r.conversation_id == conversation && !r.conversation_deleted)
    );
    for (date, currency) in [(&audit_day[..], "EUR"), ("0001-01-01", "USD")] {
        let empty = store
            .audit_reply_money(&owner, date, currency, None)
            .await
            .unwrap();
        assert!(empty.consistent);
        assert_eq!(empty.counts.requests, 0);
        assert!(empty.items.is_empty());
    }
    let other = store
        .audit_reply_money(&foreign, &audit_day, "USD", None)
        .await
        .unwrap();
    assert_eq!(other.counts.requests, 1);
    assert_eq!(other.items[0].conversation_id, foreign_conversation);
    assert!(matches!(
        store
            .audit_reply_money(&UserId::new(id()), &audit_day, "USD", None)
            .await,
        Err(StorageError::NotFound)
    ));
    for cursor in ["invalid", "uuid/uuid", ""] {
        assert!(matches!(
            store
                .audit_reply_money(&owner, &audit_day, "USD", Some(cursor))
                .await,
            Err(StorageError::InvalidData(_))
        ));
    }
    assert!(matches!(
        store
            .audit_reply_money(&owner, "2026-02-29", "USD", None)
            .await,
        Err(StorageError::InvalidData(_))
    ));
    store
        .delete_conversation(&owner, &conversation)
        .await
        .unwrap();
    // 模拟墓碑已清理，凭据仍可核对；不依赖正文或 conversation_replies 存活。
    sqlx::query("DELETE FROM conversations WHERE id=$1")
        .bind(Uuid::parse_str(&conversation).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let deleted = store
        .audit_reply_money(&owner, &audit_day, "USD", None)
        .await
        .unwrap();
    assert!(deleted.consistent);
    assert_eq!(deleted.counts.requests, 5);
    assert_eq!(deleted.counts.cancelled_before_dispatch, 2);
    assert_eq!(deleted.totals.pending_micro, "0");
    assert_eq!(deleted.ledger_occupied_micro.as_deref(), Some("2348"));
    assert!(
        deleted
            .items
            .iter()
            .all(|r| r.conversation_deleted && r.reply_status.is_none())
    );
    cleanup(&pool, &owner).await;
    cleanup(&foreign_pool, &foreign).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn audit_detects_mismatch_and_missing_daily_row_without_repairing() {
    let (store, pool, owner, conversation) = fixture().await;
    let request = id();
    reserve(&store, &owner, &conversation, &request, 10000)
        .await
        .unwrap();
    let audit_day = day(&pool, &request).await;
    let owner_id = Uuid::parse_str(owner.as_str()).unwrap();
    sqlx::query("UPDATE reply_money_daily SET occupied=occupied+1 WHERE user_id=$1")
        .bind(owner_id)
        .execute(&pool)
        .await
        .unwrap();
    let report = store
        .audit_reply_money(&owner, &audit_day, "USD", None)
        .await
        .unwrap();
    assert!(!report.consistent);
    assert_eq!(report.difference_micro, "1");
    assert_eq!(money(&pool, &owner).await, 1125);
    sqlx::query("DELETE FROM reply_money_daily WHERE user_id=$1")
        .bind(owner_id)
        .execute(&pool)
        .await
        .unwrap();
    let missing = store
        .audit_reply_money(&owner, &audit_day, "USD", None)
        .await
        .unwrap();
    assert!(!missing.consistent);
    assert!(missing.ledger_occupied_micro.is_none());
    assert_eq!(missing.difference_micro, "-1124");
    // 全额退款后，即使应占用为 0，缺失日账本仍然是异常。
    sqlx::query("UPDATE reply_money_reservations SET charged=0,settlement='cancelled_before_dispatch',settled_at=clock_timestamp() WHERE user_id=$1").bind(owner_id).execute(&pool).await.unwrap();
    let missing_zero = store
        .audit_reply_money(&owner, &audit_day, "USD", None)
        .await
        .unwrap();
    assert!(!missing_zero.consistent);
    assert_eq!(missing_zero.difference_micro, "0");
    cleanup(&pool, &owner).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn audit_pages_all_receipts_and_keeps_totals_larger_than_i64_exact() {
    let (store, pool, owner, conversation) = fixture().await;
    let owner_id = Uuid::parse_str(owner.as_str()).unwrap();
    let requests: Vec<_> = (1..=205).map(Uuid::from_u128).collect();
    sqlx::query("INSERT INTO reply_money_daily(user_id,day,currency,occupied) VALUES($1,'2030-01-01','USD',0)").bind(owner_id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO reply_money_reservations(user_id,conversation_id,request_id,day,currency,model,configuration_revision,budget,reserved,charged,settlement,settled_at) SELECT $1,$2,request,'2030-01-01','USD','local-fixture','v1','{}'::jsonb,$4,0,'cancelled_before_dispatch',clock_timestamp() FROM unnest($3::uuid[]) request")
        .bind(owner_id).bind(Uuid::parse_str(&conversation).unwrap()).bind(&requests).bind(i64::MAX).execute(&pool).await.unwrap();
    let expected = (205_u128 * u128::try_from(i64::MAX).unwrap()).to_string();
    let mut cursor = None;
    let mut seen = Vec::new();
    for size in [100, 100, 5] {
        let report = store
            .audit_reply_money(&owner, "2030-01-01", "USD", cursor.as_deref())
            .await
            .unwrap();
        assert!(report.consistent);
        assert_eq!(report.items.len(), size);
        assert_eq!(report.counts.requests, 205);
        assert_eq!(report.totals.reserved_micro, expected);
        assert_eq!(report.totals.refunded_micro, expected);
        assert_eq!(report.totals.expected_occupied_micro, "0");
        assert_eq!(report.items[0].reserved_micro, i64::MAX.to_string());
        seen.extend(
            report
                .items
                .into_iter()
                .map(|r| Uuid::parse_str(&r.request_id).unwrap()),
        );
        cursor = report.next_cursor;
    }
    assert!(cursor.is_none());
    assert_eq!(seen, requests);
    cleanup(&pool, &owner).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn configuration_queries_page_metadata_and_observe_expiry_and_durable_disable() {
    use personal_ai_storage::reply_budgets::ReplyDispatchStore;
    let (store, pool, owner, _) = fixture().await;
    let prefix = format!("zz-operations-{}-", id());
    let revisions: Vec<_> = (0..205).map(|n| format!("{prefix}{n:03}")).collect();
    sqlx::query("INSERT INTO reply_configurations(revision,model,budget,valid_until_ms) SELECT revision,'local-fixture',$2::text::jsonb,4102444800000 FROM unnest($1::text[]) revision")
        .bind(&revisions).bind(serde_json::to_string(&planner(10000).0).unwrap()).execute(&pool).await.unwrap();
    let active = store.get_reply_configuration(&revisions[0]).await.unwrap();
    assert!(active.active);
    assert_eq!(active.budget, planner(10000).0);
    assert!(active.disabled_at_unix_ms.is_none());
    sqlx::query("UPDATE reply_configurations SET valid_until_ms=1 WHERE revision=$1")
        .bind(&revisions[1])
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        !store
            .get_reply_configuration(&revisions[1])
            .await
            .unwrap()
            .active
    );
    store
        .disable_reply_configuration(&revisions[2])
        .await
        .unwrap();
    let reopened = PostgresStore::connect_existing(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let disabled = reopened
        .get_reply_configuration(&revisions[2])
        .await
        .unwrap();
    assert!(!disabled.active);
    assert!(disabled.disabled_at_unix_ms.is_some());
    assert!(matches!(
        store.get_reply_configuration(&id()).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.list_reply_configurations(Some("")).await,
        Err(StorageError::InvalidData(_))
    ));
    let mut after = Some(prefix);
    let mut seen = Vec::new();
    loop {
        let page = store
            .list_reply_configurations(after.as_deref())
            .await
            .unwrap();
        assert!(page.items.len() <= 100);
        seen.extend(
            page.items
                .into_iter()
                .filter(|r| revisions.contains(&r.configuration.revision))
                .map(|r| r.configuration.revision),
        );
        after = page.next_cursor;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(seen, revisions);
    sqlx::query("DELETE FROM reply_configurations WHERE revision=ANY($1::text[])")
        .bind(revisions)
        .execute(&pool)
        .await
        .unwrap();
    cleanup(&pool, &owner).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn audit_snapshot_remains_consistent_while_requests_settle() {
    let (store, pool, owner, conversation) = fixture().await;
    for _ in 0..10 {
        let request = id();
        reserve(&store, &owner, &conversation, &request, 10000)
            .await
            .unwrap();
        let audit_day = day(&pool, &request).await;
        let (audit, cancelled) = tokio::join!(
            store.audit_reply_money(&owner, &audit_day, "USD", None),
            store.cancel_reply(&owner, &conversation, &request)
        );
        cancelled.unwrap();
        let report = audit.unwrap();
        assert!(report.consistent);
        assert_eq!(
            report.items.len(),
            usize::try_from(report.counts.requests).unwrap()
        );
        let sum: i64 = report
            .items
            .iter()
            .map(|r| {
                r.charged_micro
                    .as_ref()
                    .unwrap_or(&r.reserved_micro)
                    .parse::<i64>()
                    .unwrap()
            })
            .sum();
        assert_eq!(report.totals.expected_occupied_micro, sum.to_string());
    }
    cleanup(&pool, &owner).await;
}
