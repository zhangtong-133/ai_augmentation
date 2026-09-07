use personal_ai_domain::{User, UserId};
use personal_ai_storage::{MetadataStore, StorageError};
use personal_ai_storage_postgres::PostgresStore;
use uuid::Uuid;

// Requires a disposable PostgreSQL database. CI provisions a dedicated service.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing at a disposable PostgreSQL database"]
async fn migrations_persistence_and_case_insensitive_uniqueness() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
    let store = PostgresStore::connect(&url).await.unwrap();
    store.health().await.unwrap();
    let user = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@example.com", Uuid::new_v4()),
        display_name: "持久化测试".into(),
    };
    store.save_user(&user).await.unwrap();
    store.set_password(&user.id, "test-hash-v1").await.unwrap();
    let token_digest = Uuid::new_v4().to_string();
    store
        .create_session(&user.id, &token_digest, "test-hash-v1")
        .await
        .unwrap();
    assert_eq!(store.session_user(&token_digest).await.unwrap(), user);
    store.delete_session(&token_digest).await.unwrap();
    assert!(matches!(
        store.session_user(&token_digest).await,
        Err(StorageError::NotFound)
    ));
    store
        .create_session(&user.id, &token_digest, "test-hash-v1")
        .await
        .unwrap();
    store.set_password(&user.id, "test-hash-v2").await.unwrap();
    assert!(matches!(
        store.session_user(&token_digest).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store
            .create_session(&user.id, &token_digest, "test-hash-v1")
            .await,
        Err(StorageError::NotFound)
    ));
    store
        .create_session(&user.id, &token_digest, "test-hash-v2")
        .await
        .unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("UPDATE sessions SET expires_at=NOW() - INTERVAL '1 second' WHERE token_digest=$1")
        .bind(&token_digest)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store.session_user(&token_digest).await,
        Err(StorageError::NotFound)
    ));
    // Reconnect and rerun migrations to verify restart safety and persistence.
    let reopened = PostgresStore::connect(&url).await.unwrap();
    assert_eq!(reopened.get_user(&user.id).await.unwrap(), user);
    let duplicate = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: user.email.to_uppercase(),
        ..user.clone()
    };
    assert!(matches!(
        reopened.save_user(&duplicate).await,
        Err(StorageError::Conflict(_))
    ));
    assert!(matches!(
        store
            .get_user(&UserId::new(Uuid::new_v4().to_string()))
            .await,
        Err(StorageError::NotFound)
    ));
}
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing at a disposable PostgreSQL database"]
#[allow(clippy::too_many_lines)] // One persistence/isolation lifecycle including aggregate boundaries.
async fn documents_persist_and_are_owner_scoped() {
    use personal_ai_storage::documents::{DocumentStore, DocumentSummary, StoredDocument};
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL required");
    let store = PostgresStore::connect(&url).await.unwrap();
    let owner = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@example.com", Uuid::new_v4()),
        display_name: "Owner".into(),
    };
    let other = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@example.com", Uuid::new_v4()),
        display_name: "Other".into(),
    };
    store.save_user(&owner).await.unwrap();
    store.save_user(&other).await.unwrap();
    let document = StoredDocument {
        summary: DocumentSummary {
            id: Uuid::new_v4().to_string(),
            title: "中文文档".into(),
            source: "note.md".into(),
            tags: vec!["Rust".into()],
            created_at_unix_ms: 1234,
            chunk_count: 1,
        },
        markdown: "# 中文".into(),
        chunks: vec!["中文".into()],
    };
    store
        .insert_document(&owner.id, "test-digest", &document)
        .await
        .unwrap();
    let reopened = PostgresStore::connect(&url).await.unwrap();
    let stats = reopened
        .document_stats(&owner.id, 1234, 1235)
        .await
        .unwrap();
    assert_eq!(stats.total_documents, 1);
    assert_eq!(stats.total_chunks, 1);
    assert_eq!(stats.imported_today, 1);
    assert_eq!(
        reopened
            .document_stats(&owner.id, 0, 1234)
            .await
            .unwrap()
            .imported_today,
        0
    );
    assert_eq!(
        reopened
            .document_stats(&owner.id, 1235, 9999)
            .await
            .unwrap()
            .imported_today,
        0
    );
    assert_eq!(
        reopened.document_stats(&other.id, 0, 9999).await.unwrap(),
        personal_ai_storage::documents::DocumentStats::default()
    );
    assert_eq!(
        reopened
            .get_document(&owner.id, &document.summary.id)
            .await
            .unwrap(),
        document
    );
    assert_eq!(
        reopened.list_documents(&owner.id, 0).await.unwrap(),
        vec![document.summary.clone()]
    );
    assert!(
        reopened
            .list_documents(&other.id, 0)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        reopened.get_document(&other.id, &document.summary.id).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        reopened
            .insert_document(&owner.id, "test-digest", &document)
            .await,
        Err(StorageError::Conflict(_))
    ));
    let second = StoredDocument {
        summary: DocumentSummary {
            id: Uuid::new_v4().to_string(),
            ..document.summary
        },
        ..document
    };
    reopened
        .insert_document(&other.id, "test-digest", &second)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .get_document(&other.id, &second.summary.id)
            .await
            .unwrap(),
        second
    );
}
