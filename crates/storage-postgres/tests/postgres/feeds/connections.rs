use super::*;
use personal_ai_storage::subscription_connections::{
    SubscriptionConnectionStore, VerifiedSubscriptionConnection,
};
async fn input(f: &Fixture) -> VerifiedSubscriptionConnection {
    let time: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    VerifiedSubscriptionConnection {
        host_id: Uuid::new_v4().urn().to_string(),
        client_id: format!("oaiapp_{}", Uuid::new_v4().simple()),
        subject: "private-subject".into(),
        label: "personal".into(),
        models: vec!["fixture".into()],
        valid_until_unix_ms: time + 3_000_000,
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn connections_bind_identity_owner_and_revision_without_resurrecting_old_requests() {
    let f = Fixture::new().await;
    let id = Uuid::new_v4().to_string();
    let mut proof = input(&f).await;
    let (a, b) = tokio::join!(
        f.store
            .save_subscription_connection(&f.owner, &id, 0, &proof),
        f.store
            .save_subscription_connection(&f.owner, &id, 0, &proof)
    );
    let saved = a.unwrap();
    assert_eq!(saved, b.unwrap());
    assert_eq!(saved.revision, 1);
    assert!(
        !serde_json::to_string(&saved)
            .unwrap()
            .contains("private-subject")
    );
    assert!(matches!(
        f.store.get_subscription_connection(&f.other, &id).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store
            .revoke_subscription_connection(&f.other, &id, 1)
            .await,
        Err(StorageError::NotFound)
    ));
    assert_eq!(
        f.store
            .list_subscription_connections(&f.other, None)
            .await
            .unwrap()
            .items,
        [] as [personal_ai_storage::subscription_connections::SubscriptionConnection; 0]
    );
    assert!(
        f.store
            .save_subscription_connection(&f.other, &Uuid::new_v4().to_string(), 0, &proof)
            .await
            .is_err()
    );
    let revoked = f
        .store
        .revoke_subscription_connection(&f.owner, &id, 1)
        .await
        .unwrap();
    assert_eq!(revoked.status, "revoked");
    assert_eq!(revoked.revision, 2);
    assert_eq!(
        revoked,
        f.store
            .revoke_subscription_connection(&f.owner, &id, 1)
            .await
            .unwrap()
    );
    assert!(
        f.store
            .save_subscription_connection(&f.owner, &id, 0, &proof)
            .await
            .is_err()
    );
    let revived = f
        .store
        .save_subscription_connection(&f.owner, &id, 2, &proof)
        .await
        .unwrap();
    assert_eq!(revived.revision, 3);
    assert!(
        f.store
            .revoke_subscription_connection(&f.owner, &id, 1)
            .await
            .is_err()
    );
    proof.subject = "another-subject".into();
    assert!(
        f.store
            .save_subscription_connection(&f.owner, &id, 3, &proof)
            .await
            .is_err()
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM subscription_connection_audit WHERE user_id=$1")
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(count, 3);
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .get_subscription_connection(&f.owner, &id)
            .await
            .unwrap(),
        revived
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn connection_quota_pagination_and_expiration_are_persistent() {
    let f = Fixture::new().await;
    let mut ids = Vec::new();
    for _ in 0..20 {
        let id = Uuid::new_v4().to_string();
        f.store
            .save_subscription_connection(&f.owner, &id, 0, &input(&f).await)
            .await
            .unwrap();
        ids.push(id);
    }
    let first = f
        .store
        .list_subscription_connections(&f.owner, None)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 10);
    let second = f
        .store
        .list_subscription_connections(&f.owner, first.next_cursor.as_deref())
        .await
        .unwrap();
    assert_eq!(second.items.len(), 10);
    assert!(second.next_cursor.is_none());
    assert!(first.items.last().unwrap().id < second.items.first().unwrap().id);
    f.store
        .revoke_subscription_connection(&f.owner, &ids[0], 1)
        .await
        .unwrap();
    assert!(
        f.store
            .save_subscription_connection(
                &f.owner,
                &Uuid::new_v4().to_string(),
                0,
                &input(&f).await
            )
            .await
            .is_err()
    );
    sqlx::query("UPDATE subscription_connections SET expires_ms=1 WHERE user_id=$1 AND id=$2")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .bind(Uuid::parse_str(&ids[1]).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    let expired = f
        .store
        .get_subscription_connection(&f.owner, &ids[1])
        .await
        .unwrap();
    assert_eq!(expired.status, "expired");
    assert_eq!(expired.revision, 2);
    assert_eq!(
        f.store
            .get_subscription_connection(&f.owner, &ids[1])
            .await
            .unwrap(),
        expired
    );
    f.cleanup().await;
}
