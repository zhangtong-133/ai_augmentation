use super::*;
use personal_ai_storage::{
    StorageResult, briefs::BriefStore, feed_value::*, model_agents::ModelCallBudget,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
struct Planner {
    subscription: bool,
    calls: AtomicUsize,
}
impl ValueQuotePlanner for Planner {
    fn quote(
        &self,
        owner: &UserId,
        _request: &str,
        snapshot: &ValueSnapshot,
    ) -> StorageResult<ValuePricing> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let until = i64::try_from(snapshot.as_of_unix_ms).unwrap() + 600_000;
        Ok(if self.subscription {
            ValuePricing::Subscription {
                provider: "chatgpt-plan".into(),
                model: "fixture".into(),
                configuration_version: "connection-v1".into(),
                connection_id: owner.as_str().into(),
                valid_until_unix_ms: until,
            }
        } else {
            ValuePricing::Api {
                budget: ModelCallBudget {
                    configuration_version: "v1".into(),
                    provider: "fixture".into(),
                    model: "fixture".into(),
                    price_version: "v1".into(),
                    counter_version: "v1".into(),
                    currency: "USD".into(),
                    input_price_per_million: 1_000_000,
                    output_price_per_million: 1_000_000,
                    input_token_bound: 1000,
                    output_token_bound: 4096,
                    valid_until_unix_ms: until,
                },
                request_limit: 6000,
            }
        })
    }
}
fn planner(subscription: bool) -> Arc<Planner> {
    Arc::new(Planner {
        subscription,
        calls: AtomicUsize::new(0),
    })
}
async fn setup() -> (Fixture, Subscription) {
    use personal_ai_storage::subscription_connections::{
        SubscriptionConnectionStore, VerifiedSubscriptionConnection,
    };
    let f = Fixture::new().await;
    let time: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    f.store
        .save_subscription_connection(
            &f.owner,
            f.owner.as_str(),
            0,
            &VerifiedSubscriptionConnection {
                host_id: Uuid::new_v4().urn().to_string(),
                client_id: format!("oaiapp_{}", Uuid::new_v4().simple()),
                subject: "fixture".into(),
                label: "fixture".into(),
                models: vec!["fixture".into()],
                valid_until_unix_ms: time + 3_000_000,
            },
        )
        .await
        .unwrap();
    let sub = f.sub().await;
    let claim = f.claim(&sub).await;
    f.store
        .finish_collection(&claim, response(ITEM))
        .await
        .unwrap();
    f.store
        .save_brief_preferences(&f.owner, 0, &["Rust".into()])
        .await
        .unwrap();
    (f, sub)
}
fn approval(saved: &ValueReview) -> ValueApproval {
    ValueApproval {
        digest: saved.digest.clone(),
        currency: match &saved.pricing {
            ValuePricing::Api { budget, .. } => Some(budget.currency.clone()),
            ValuePricing::Subscription { .. } => None,
        },
        amount: saved.amount,
        acknowledge_sharing: true,
        acknowledge_cost: matches!(saved.pricing, ValuePricing::Api { .. }),
        acknowledge_subscription_usage: matches!(saved.pricing, ValuePricing::Subscription { .. }),
    }
}
async fn preview(f: &Fixture, p: Arc<Planner>) -> ValueReview {
    f.store
        .preview_feed_value(&f.owner, &Uuid::new_v4().to_string(), p)
        .await
        .unwrap()
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn value_reviews_bind_sharing_and_funding_without_reserving_money() {
    let (f, _) = setup().await;
    for subscription in [false, true] {
        let p = planner(subscription);
        let saved = preview(&f, p.clone()).await;
        let id = &saved.request_id;
        assert_eq!(saved.amount, if subscription { None } else { Some(5096) });
        let again = f
            .store
            .preview_feed_value(&f.owner, id, p.clone())
            .await
            .unwrap();
        assert!(again == saved);
        assert_eq!(p.calls.load(Ordering::SeqCst), 1);
        let consent = approval(&saved);
        for variant in 0..5 {
            let mut wrong = consent.clone();
            match variant {
                0 => wrong.acknowledge_sharing = false,
                1 => wrong.digest = "0".repeat(64),
                2 => wrong.amount = Some(1),
                3 => wrong.currency = Some("EUR".into()),
                _ => {
                    wrong.acknowledge_cost = !wrong.acknowledge_cost;
                    wrong.acknowledge_subscription_usage = !wrong.acknowledge_subscription_usage;
                }
            }
            is_conflict(f.store.approve_feed_value(&f.owner, id, &wrong).await);
        }
        let (a, b) = tokio::join!(
            f.store.approve_feed_value(&f.owner, id, &consent),
            f.store.approve_feed_value(&f.owner, id, &consent)
        );
        let a = a.unwrap();
        assert!(a == b.unwrap());
        assert_eq!(a.status, "authorized");
        assert!(matches!(
            f.store.get_feed_value(&f.other, id).await,
            Err(StorageError::NotFound)
        ));
        assert!(matches!(
            f.store.approve_feed_value(&f.other, id, &consent).await,
            Err(StorageError::NotFound)
        ));
        assert!(matches!(
            f.store.cancel_feed_value(&f.other, id).await,
            Err(StorageError::NotFound)
        ));
        assert!(matches!(
            f.store.feed_value_audit(&f.other, id).await,
            Err(StorageError::NotFound)
        ));
        let cancelled = f.store.cancel_feed_value(&f.owner, id).await.unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert!(cancelled.snapshot.is_none());
        assert!(f.store.cancel_feed_value(&f.owner, id).await.unwrap() == cancelled);
        is_conflict(f.store.approve_feed_value(&f.owner, id, &consent).await);
        assert_eq!(
            f.store
                .feed_value_audit(&f.owner, id)
                .await
                .unwrap()
                .iter()
                .map(|a| a.event.as_str())
                .collect::<Vec<_>>(),
            vec!["draft", "authorized", "cancelled"]
        );
    }
    assert!(
        f.store
            .list_feed_values(&f.other, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let reservations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reply_money_daily WHERE user_id=$1")
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(reservations, 0);
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn value_reviews_invalidate_and_scrub_on_source_entry_or_preference_changes() {
    let (f, sub) = setup().await;
    for change in 0..3 {
        let saved = preview(&f, planner(true)).await;
        f.store
            .approve_feed_value(&f.owner, &saved.request_id, &approval(&saved))
            .await
            .unwrap();
        let owner = Uuid::parse_str(f.owner.as_str()).unwrap();
        match change {
            0 => {
                sqlx::query("UPDATE feed_entries SET summary='changed' WHERE user_id=$1")
                    .bind(owner)
                    .execute(&f.pool)
                    .await
                    .unwrap();
            }
            1 => {
                f.store
                    .save_brief_preferences(&f.owner, 1, &["changed".into()])
                    .await
                    .unwrap();
            }
            _ => {
                f.store
                    .delete_subscription(
                        &f.owner,
                        &sub.snapshot.subscription_id,
                        sub.snapshot.revision,
                    )
                    .await
                    .unwrap();
            }
        }
        let result = f
            .store
            .get_feed_value(&f.owner, &saved.request_id)
            .await
            .unwrap();
        assert_eq!(result.status, "invalidated");
        assert!(result.snapshot.is_none());
        is_conflict(
            f.store
                .approve_feed_value(&f.owner, &saved.request_id, &approval(&saved))
                .await,
        );
    }
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn value_review_expiration_and_cancellation_win_approval_races() {
    let (f, _) = setup().await;
    let saved = preview(&f, planner(false)).await;
    let consent = approval(&saved);
    let (a, b) = tokio::join!(
        f.store
            .approve_feed_value(&f.owner, &saved.request_id, &consent),
        f.store.cancel_feed_value(&f.owner, &saved.request_id)
    );
    assert!(a.is_ok() || matches!(a, Err(StorageError::Conflict(_))));
    assert_eq!(b.unwrap().status, "cancelled");
    assert_eq!(
        f.store
            .get_feed_value(&f.owner, &saved.request_id)
            .await
            .unwrap()
            .status,
        "cancelled"
    );
    let saved = preview(&f, planner(true)).await;
    sqlx::query("UPDATE feed_value_reviews SET expires_ms=created_ms+1 WHERE user_id=$1 AND id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .bind(Uuid::parse_str(&saved.request_id).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    let expired = f
        .store
        .get_feed_value(&f.owner, &saved.request_id)
        .await
        .unwrap();
    assert_eq!(expired.status, "expired");
    assert!(expired.snapshot.is_none());
    is_conflict(
        f.store
            .approve_feed_value(&f.owner, &saved.request_id, &approval(&saved))
            .await,
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn value_reviews_quota_and_pagination_survive_cancellation() {
    let (f, _) = setup().await;
    let p = planner(true);
    for _ in 0..20 {
        let saved = preview(&f, p.clone()).await;
        f.store
            .cancel_feed_value(&f.owner, &saved.request_id)
            .await
            .unwrap();
    }
    is_conflict(
        f.store
            .preview_feed_value(&f.owner, &Uuid::new_v4().to_string(), p.clone())
            .await,
    );
    assert_eq!(p.calls.load(Ordering::SeqCst), 20);
    sqlx::query("UPDATE feed_value_reviews SET created_ms=created_ms-86400000,expires_ms=expires_ms-86400000 WHERE user_id=$1").bind(Uuid::parse_str(f.owner.as_str()).unwrap()).execute(&f.pool).await.unwrap();
    preview(&f, p).await;
    let page = f.store.list_feed_values(&f.owner, None).await.unwrap();
    assert_eq!(page.items.len(), 20);
    let last = f
        .store
        .list_feed_values(&f.owner, page.next_cursor.as_deref())
        .await
        .unwrap();
    assert_eq!(last.items.len(), 1);
    assert!(last.next_cursor.is_none());
    let restored = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert!(
        restored
            .get_feed_value(&f.owner, &page.items[0].request_id)
            .await
            .unwrap()
            == page.items[0]
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn value_review_audit_failure_rolls_back_authorization() {
    let (f, _) = setup().await;
    let saved = preview(&f, planner(false)).await;
    let constraint = format!("value_fail_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("ALTER TABLE feed_value_audit ADD CONSTRAINT {constraint} CHECK(user_id <> '{}'::uuid) NOT VALID",f.owner.as_str())).execute(&f.pool).await.unwrap();
    assert!(
        f.store
            .approve_feed_value(&f.owner, &saved.request_id, &approval(&saved))
            .await
            .is_err()
    );
    sqlx::query(&format!(
        "ALTER TABLE feed_value_audit DROP CONSTRAINT {constraint}"
    ))
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        f.store
            .get_feed_value(&f.owner, &saved.request_id)
            .await
            .unwrap()
            .status,
        "draft"
    );
    assert_eq!(
        f.store
            .feed_value_audit(&f.owner, &saved.request_id)
            .await
            .unwrap()
            .len(),
        1
    );
    f.cleanup().await;
}

struct ConnectionPlanner(ValuePricing);
impl ValueQuotePlanner for ConnectionPlanner {
    fn quote(&self, _: &UserId, _: &str, _: &ValueSnapshot) -> StorageResult<ValuePricing> {
        Ok(self.0.clone())
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn subscription_reviews_reject_unknown_foreign_stale_models_and_revoke_atomically() {
    use personal_ai_storage::subscription_connections::SubscriptionConnectionStore;
    let (f, _) = setup().await;
    let saved = preview(&f, planner(true)).await;
    for field in ["connection", "version", "model", "provider", "expiry"] {
        let mut pricing = saved.pricing.clone();
        if let ValuePricing::Subscription {
            connection_id,
            configuration_version,
            model,
            provider,
            valid_until_unix_ms,
        } = &mut pricing
        {
            match field {
                "connection" => *connection_id = f.other.as_str().into(),
                "version" => *configuration_version = "connection-v2".into(),
                "model" => *model = "not-in-catalog".into(),
                "provider" => *provider = "api-key".into(),
                _ => *valid_until_unix_ms += 3_600_000,
            }
        }
        assert!(
            f.store
                .preview_feed_value(
                    &f.owner,
                    &Uuid::new_v4().to_string(),
                    Arc::new(ConnectionPlanner(pricing))
                )
                .await
                .is_err(),
            "{field}"
        );
    }
    let api = preview(&f, planner(false)).await;
    let consent = approval(&saved);
    let (revoked, _) = tokio::join!(
        f.store
            .revoke_subscription_connection(&f.owner, f.owner.as_str(), 1),
        f.store
            .approve_feed_value(&f.owner, &saved.request_id, &consent)
    );
    assert_eq!(revoked.unwrap().status, "revoked");
    let invalid = f
        .store
        .get_feed_value(&f.owner, &saved.request_id)
        .await
        .unwrap();
    assert_eq!(invalid.status, "invalidated");
    assert!(invalid.snapshot.is_none());
    assert!(
        f.store
            .approve_feed_value(&f.owner, &saved.request_id, &consent)
            .await
            .is_err()
    );
    assert_eq!(
        f.store
            .get_feed_value(&f.owner, &api.request_id)
            .await
            .unwrap()
            .status,
        "draft"
    );
    assert_eq!(
        f.store
            .feed_value_audit(&f.owner, &saved.request_id)
            .await
            .unwrap()
            .last()
            .unwrap()
            .event,
        "invalidated"
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn subscription_revocation_rolls_back_when_review_audit_fails() {
    use personal_ai_storage::subscription_connections::SubscriptionConnectionStore;
    let (f, _) = setup().await;
    let saved = preview(&f, planner(true)).await;
    let name = format!("reject_connection_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("ALTER TABLE feed_value_audit ADD CONSTRAINT {name} CHECK(user_id<>'{}' OR event<>'invalidated') NOT VALID",f.owner.as_str())).execute(&f.pool).await.unwrap();
    assert!(
        f.store
            .revoke_subscription_connection(&f.owner, f.owner.as_str(), 1)
            .await
            .is_err()
    );
    assert_eq!(
        f.store
            .get_subscription_connection(&f.owner, f.owner.as_str())
            .await
            .unwrap()
            .status,
        "active"
    );
    assert_eq!(
        f.store
            .get_feed_value(&f.owner, &saved.request_id)
            .await
            .unwrap()
            .status,
        "draft"
    );
    sqlx::query(&format!(
        "ALTER TABLE feed_value_audit DROP CONSTRAINT {name}"
    ))
    .execute(&f.pool)
    .await
    .unwrap();
    f.store
        .revoke_subscription_connection(&f.owner, f.owner.as_str(), 1)
        .await
        .unwrap();
    f.cleanup().await;
}

#[path = "value/execution.rs"]
mod execution;
