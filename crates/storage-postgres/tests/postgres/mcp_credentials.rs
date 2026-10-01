use super::*;
use personal_ai_storage::mcp_credentials::NewMcpCredential;

#[tokio::test]
#[allow(clippy::too_many_lines)] // Keep the credential lifecycle in one database fixture.
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn mcp_credentials_are_private_revocable_expiring_and_password_reset_invalidates() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let owner = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@mcp.example", Uuid::new_v4()),
        display_name: "MCP".into(),
    };
    store.save_user(&owner).await.unwrap();
    store.set_password(&owner.id, "initial-hash").await.unwrap();
    let session = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    store
        .create_session(&owner.id, &session, "initial-hash")
        .await
        .unwrap();
    let foreign = UserId::new(Uuid::new_v4().to_string());
    let input = NewMcpCredential {
        session_digest: session.clone(),
        digest: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
        host_name: "本地宿主".into(),
        days: 1,
    };
    let saved = store
        .create_mcp_credential(&owner.id, &input)
        .await
        .unwrap();
    assert_eq!(saved.scope, "knowledge_search");
    assert_eq!(
        store.mcp_credential_owner(&input.digest).await.unwrap(),
        owner.id
    );
    assert!(
        store
            .list_mcp_credentials(&foreign)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        store.revoke_mcp_credential(&foreign, &saved.id).await,
        Err(StorageError::NotFound)
    ));
    let reopened = PostgresStore::connect(&url).await.unwrap();
    assert_eq!(
        reopened.mcp_credential_owner(&input.digest).await.unwrap(),
        owner.id
    );
    let serialized =
        serde_json::to_string(&reopened.list_mcp_credentials(&owner.id).await.unwrap()).unwrap();
    assert!(!serialized.contains(&input.digest));
    store
        .revoke_mcp_credential(&owner.id, &saved.id)
        .await
        .unwrap();
    store
        .revoke_mcp_credential(&owner.id, &saved.id)
        .await
        .unwrap();
    assert!(matches!(
        reopened.mcp_credential_owner(&input.digest).await,
        Err(StorageError::NotFound)
    ));
    let expiring = NewMcpCredential {
        digest: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
        ..input.clone()
    };
    let saved = store
        .create_mcp_credential(&owner.id, &expiring)
        .await
        .unwrap();
    sqlx::query("UPDATE mcp_credentials SET created_at=NOW()-INTERVAL '2 days',expires_at=NOW()-INTERVAL '1 day' WHERE id=$1")
        .bind(Uuid::parse_str(&saved.id).unwrap()).execute(&pool).await.unwrap();
    assert!(matches!(
        store.mcp_credential_owner(&expiring.digest).await,
        Err(StorageError::NotFound)
    ));
    let active = NewMcpCredential {
        digest: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
        ..input
    };
    store
        .create_mcp_credential(&owner.id, &active)
        .await
        .unwrap();
    store
        .set_password(&owner.id, "replacement-hash")
        .await
        .unwrap();
    assert!(matches!(
        store.mcp_credential_owner(&active.digest).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.create_mcp_credential(&owner.id, &active).await,
        Err(StorageError::NotFound)
    ));
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.id.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn mcp_issuance_quota_is_atomic_and_revocation_does_not_reset_daily_limit() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = std::sync::Arc::new(PostgresStore::connect(&url).await.unwrap());
    let owner = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@mcp.example", Uuid::new_v4()),
        display_name: "MCP".into(),
    };
    store.save_user(&owner).await.unwrap();
    store.set_password(&owner.id, "initial-hash").await.unwrap();
    let session = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    store
        .create_session(&owner.id, &session, "initial-hash")
        .await
        .unwrap();
    let mut tasks = Vec::new();
    for _ in 0..24 {
        let store = store.clone();
        let owner = owner.id.clone();
        let session = session.clone();
        tasks.push(tokio::spawn(async move {
            store
                .create_mcp_credential(
                    &owner,
                    &NewMcpCredential {
                        session_digest: session,
                        digest: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
                        host_name: "host".into(),
                        days: 30,
                    },
                )
                .await
        }));
    }
    let mut successes = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(saved) => {
                successes += 1;
                store
                    .revoke_mcp_credential(&owner.id, &saved.id)
                    .await
                    .unwrap();
            }
            Err(StorageError::Conflict(_)) => {}
            _ => panic!("unexpected issuance outcome"),
        }
    }
    assert_eq!(successes, 20);
    assert_eq!(
        store.list_mcp_credentials(&owner.id).await.unwrap().len(),
        20
    );
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.id.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
}
