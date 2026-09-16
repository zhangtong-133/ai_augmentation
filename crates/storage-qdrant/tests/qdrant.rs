use personal_ai_domain::UserId;
use personal_ai_storage::{EmbeddingRecord, StorageError, VectorStore};
use personal_ai_storage_qdrant::QdrantStore;
use uuid::Uuid;
fn record(model: &str) -> EmbeddingRecord {
    EmbeddingRecord {
        id: "shared-id".into(),
        document_id: Uuid::new_v4().to_string(),
        ordinal: 0,
        text: "中文分块".into(),
        model: model.into(),
        vector: vec![1.0, 0.0, 0.0],
        source: "note.md".into(),
        content_type: "markdown".into(),
        created_at_unix_ms: 1,
        tags: vec![],
    }
}
#[tokio::test]
#[ignore = "requires disposable Qdrant; run make smoke-index"]
async fn persistent_vectors_are_idempotent_and_owner_and_model_scoped() {
    let url = std::env::var("TEST_QDRANT_URL").unwrap();
    let collection = format!("test_{}", Uuid::new_v4().simple());
    let store = QdrantStore::new(&url, &collection, None, "m", 3).unwrap();
    store.ensure_collection().await.unwrap();
    store.ensure_collection().await.unwrap();
    assert!(matches!(
        QdrantStore::new(&url, &collection, None, "m", 2)
            .unwrap()
            .ensure_collection()
            .await,
        Err(StorageError::InvalidData(_))
    ));
    let owner = UserId::new(Uuid::new_v4().to_string());
    let other = UserId::new(Uuid::new_v4().to_string());
    let mut doc = record("m");
    store
        .insert_embeddings(&owner, &[doc.clone()])
        .await
        .unwrap();
    doc.text = "更新分块".into();
    store
        .insert_embeddings(&owner, &[doc.clone()])
        .await
        .unwrap();
    assert!(
        store
            .similar_search(&other, &[1.0, 0.0, 0.0], 20)
            .await
            .unwrap()
            .is_empty()
    );
    let reopened = QdrantStore::new(&url, &collection, None, "m", 3).unwrap();
    let found = reopened
        .similar_search(&owner, &[1.0, 0.0, 0.0], 20)
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].record.text, "更新分块");
    store
        .insert_embeddings(&other, &[record("m")])
        .await
        .unwrap();
    let other_model = QdrantStore::new(&url, &collection, None, "other-model", 3).unwrap();
    other_model
        .insert_embeddings(&owner, &[record("other-model")])
        .await
        .unwrap();
    assert_eq!(
        store
            .similar_search(&owner, &[1.0, 0.0, 0.0], 20)
            .await
            .unwrap()
            .len(),
        1
    );
    store.remove(&owner, &[doc.id]).await.unwrap();
    assert!(
        store
            .similar_search(&owner, &[1.0, 0.0, 0.0], 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .similar_search(&other, &[1.0, 0.0, 0.0], 20)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        other_model
            .similar_search(&owner, &[1.0, 0.0, 0.0], 20)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .similar_search(&owner, &[0.0, 0.0, 0.0], 20)
            .await
            .is_err()
    );
    assert!(
        store
            .insert_embeddings(&owner, &[record("wrong-model")])
            .await
            .is_err()
    );
}
