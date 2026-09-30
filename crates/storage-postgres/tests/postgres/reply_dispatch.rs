use super::*;
use personal_ai_storage::reply_budgets::ReplyDispatchStore;

const UNTIL: i64 = 4_102_444_800_000;
struct DispatchPlanner;
impl ReplyBudgetPlanner for DispatchPlanner {
    fn plan(&self, _: &ReplyContext) -> StorageResult<ReplyBudget> {
        Ok(planner(10000).0.clone())
    }
}
fn config() -> ReplyConfiguration {
    ReplyConfiguration {
        model: "local-fixture".into(),
        revision: id(),
    }
}
async fn queue(
    store: &PostgresStore,
    owner: &UserId,
    conversation: &str,
    config: &ReplyConfiguration,
) -> String {
    let request = id();
    store
        .reserve_budgeted_reply(
            owner,
            conversation,
            &request,
            1,
            config,
            Arc::new(DispatchPlanner),
        )
        .await
        .unwrap();
    request
}
async fn register(store: &PostgresStore, config: &ReplyConfiguration) {
    store
        .register_reply_configuration(config, &planner(10000).0, UNTIL)
        .await
        .unwrap();
}
async fn clean_config(pool: &sqlx::PgPool, config: &ReplyConfiguration) {
    sqlx::query("DELETE FROM reply_configurations WHERE revision=$1")
        .bind(&config.revision)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn budget_claim_is_once_only_committed_and_owner_scoped() {
    let (store, pool, owner, conversation) = fixture().await;
    let config = config();
    register(&store, &config).await;
    let request = queue(&store, &owner, &conversation, &config).await;
    let (_, foreign_pool, foreign, _) = fixture().await;
    assert!(matches!(
        store
            .claim_budgeted_reply(&foreign, &conversation, &request)
            .await,
        Err(StorageError::NotFound)
    ));
    let (a, b) = tokio::join!(
        store.claim_budgeted_reply(&owner, &conversation, &request),
        store.claim_budgeted_reply(&owner, &conversation, &request)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let claim = a.or(b).ok().unwrap();
    assert_eq!(claim.budget, planner(10000).0);
    assert_eq!(claim.reserved, 1124);
    assert_eq!(claim.reply.context.unwrap().configuration, config);
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .get_reply(&owner, &conversation, &request)
            .await
            .unwrap()
            .status,
        ReplyStatus::Dispatching
    );
    assert!(
        reopened
            .claim_budgeted_reply(&owner, &conversation, &request)
            .await
            .is_err()
    );
    cleanup(&pool, &owner).await;
    cleanup(&foreign_pool, &foreign).await;
    clean_config(&pool, &config).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn configuration_is_immutable_and_disable_survives_reconnect() {
    let (store, pool, owner, conversation) = fixture().await;
    let config = config();
    register(&store, &config).await;
    register(&store, &config).await;
    let mut changed = planner(10000).0.clone();
    changed.price_version = "changed".into();
    assert!(
        store
            .register_reply_configuration(&config, &changed, UNTIL)
            .await
            .is_err()
    );
    assert!(
        store
            .register_reply_configuration(&config, &planner(10000).0, UNTIL + 1)
            .await
            .is_err()
    );
    assert!(
        store
            .check_reply_configuration(&config, &changed)
            .await
            .is_err()
    );
    let request = queue(&store, &owner, &conversation, &config).await;
    store
        .disable_reply_configuration(&config.revision)
        .await
        .unwrap();
    store
        .disable_reply_configuration(&config.revision)
        .await
        .unwrap();
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert!(
        reopened
            .register_reply_configuration(&config, &planner(10000).0, UNTIL)
            .await
            .is_err()
    );
    assert!(
        reopened
            .check_reply_configuration(&config, &planner(10000).0)
            .await
            .is_err()
    );
    assert!(
        reopened
            .claim_budgeted_reply(&owner, &conversation, &request)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .get_reply(&owner, &conversation, &request)
            .await
            .unwrap()
            .status,
        ReplyStatus::Queued
    );
    assert_eq!(money(&pool, &owner).await, 1124);
    store
        .cancel_reply(&owner, &conversation, &request)
        .await
        .unwrap();
    assert_eq!(money(&pool, &owner).await, 0);
    cleanup(&pool, &owner).await;
    clean_config(&pool, &config).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn claim_rejects_legacy_missing_expired_and_mismatched_budgets() {
    let (store, pool, owner, conversation) = fixture().await;
    let config = config();
    let legacy = id();
    store
        .reserve_reply(&owner, &conversation, &legacy, 1, &config)
        .await
        .unwrap();
    assert!(
        store
            .claim_budgeted_reply(&owner, &conversation, &legacy)
            .await
            .is_err()
    );
    store
        .cancel_reply(&owner, &conversation, &legacy)
        .await
        .unwrap();
    let request = queue(&store, &owner, &conversation, &config).await;
    assert!(
        store
            .claim_budgeted_reply(&owner, &conversation, &request)
            .await
            .is_err()
    );
    assert!(
        store
            .register_reply_configuration(&config, &planner(10000).0, 1)
            .await
            .is_err()
    );
    register(&store, &config).await;
    sqlx::query("UPDATE reply_configurations SET valid_until_ms=1 WHERE revision=$1")
        .bind(&config.revision)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        store
            .claim_budgeted_reply(&owner, &conversation, &request)
            .await
            .is_err()
    );
    sqlx::query("UPDATE reply_configurations SET valid_until_ms=$2 WHERE revision=$1")
        .bind(&config.revision)
        .bind(UNTIL)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE reply_money_reservations SET reserved=reserved+1 WHERE request_id=$1")
        .bind(Uuid::parse_str(&request).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        store
            .claim_budgeted_reply(&owner, &conversation, &request)
            .await
            .is_err()
    );
    sqlx::query("UPDATE reply_money_reservations SET reserved=reserved-1,model='changed' WHERE request_id=$1").bind(Uuid::parse_str(&request).unwrap()).execute(&pool).await.unwrap();
    assert!(
        store
            .claim_budgeted_reply(&owner, &conversation, &request)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .get_reply(&owner, &conversation, &request)
            .await
            .unwrap()
            .status,
        ReplyStatus::Queued
    );
    cleanup(&pool, &owner).await;
    clean_config(&pool, &config).await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn claim_races_with_cancel_delete_and_configuration_disable() {
    let (store, pool, owner, conversation) = fixture().await;
    let config = config();
    register(&store, &config).await;
    let request = queue(&store, &owner, &conversation, &config).await;
    let (claim, cancel) = tokio::join!(
        store.claim_budgeted_reply(&owner, &conversation, &request),
        store.cancel_reply(&owner, &conversation, &request)
    );
    cancel.unwrap();
    assert_eq!(
        money(&pool, &owner).await,
        if claim.is_ok() { 1124 } else { 0 }
    );
    let request = queue(&store, &owner, &conversation, &config).await;
    let (_, disable) = tokio::join!(
        store.claim_budgeted_reply(&owner, &conversation, &request),
        store.disable_reply_configuration(&config.revision)
    );
    disable.unwrap();
    assert!(
        store
            .check_reply_configuration(&config, &planner(10000).0)
            .await
            .is_err()
    );
    store
        .cancel_reply(&owner, &conversation, &request)
        .await
        .unwrap();
    let next = self::config();
    register(&store, &next).await;
    let request = queue(&store, &owner, &conversation, &next).await;
    let (_, deleted) = tokio::join!(
        store.claim_budgeted_reply(&owner, &conversation, &request),
        store.delete_conversation(&owner, &conversation)
    );
    deleted.unwrap();
    assert!(matches!(
        store
            .claim_budgeted_reply(&owner, &conversation, &request)
            .await,
        Err(StorageError::NotFound)
    ));
    cleanup(&pool, &owner).await;
    clean_config(&pool, &config).await;
    clean_config(&pool, &next).await;
}
