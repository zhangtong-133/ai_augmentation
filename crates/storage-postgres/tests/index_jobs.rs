use personal_ai_domain::{User, UserId};
use personal_ai_storage::{
    MetadataStore, StorageError,
    documents::{DocumentStore, DocumentSummary, StoredDocument},
    index_jobs::{IndexFailure, IndexJobStore},
};
use personal_ai_storage_postgres::PostgresStore;
use sqlx::PgPool;
use uuid::Uuid;

async fn expire(pool: &PgPool, id: &str) {
    sqlx::query("UPDATE document_index_jobs SET lease_until=NOW()-INTERVAL '1 second',available_at=NOW()-INTERVAL '1 second' WHERE id=$1")
        .bind(Uuid::parse_str(id).unwrap()).execute(pool).await.unwrap();
}
async fn ready(pool: &PgPool, id: &str) {
    sqlx::query(
        "UPDATE document_index_jobs SET available_at=NOW()-INTERVAL '1 second' WHERE id=$1",
    )
    .bind(Uuid::parse_str(id).unwrap())
    .execute(pool)
    .await
    .unwrap();
}
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing at disposable PostgreSQL"]
#[allow(clippy::too_many_lines)]
async fn durable_jobs_are_private_leased_resumable_and_bounded() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let reopened = PostgresStore::connect(&url).await.unwrap();
    let pool = PgPool::connect(&url).await.unwrap();
    let owner = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@test.example", Uuid::new_v4()),
        display_name: "Owner".into(),
    };
    store.save_user(&owner).await.unwrap();
    let other = UserId::new(Uuid::new_v4().to_string());
    let doc = StoredDocument {
        summary: DocumentSummary {
            id: Uuid::new_v4().to_string(),
            title: "任务".into(),
            source: "test".into(),
            source_type: "markdown".into(),
            tags: vec![],
            created_at_unix_ms: 1,
            chunk_count: 17,
        },
        markdown: "原文".into(),
        original_pdf: None,
        original_html: None,
        chunks: vec!["分块".into(); 17],
    };
    store
        .insert_document(&owner.id, "job-digest", &doc)
        .await
        .unwrap();
    // 每个用例使用独立配置，避免并行测试领取其他测试的任务。
    let profile = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let doc_id = &doc.summary.id;
    assert!(
        store
            .index_status(&owner.id, doc_id, &profile)
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        store.index_status(&other, doc_id, &profile).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.enqueue_index(&other, doc_id, &profile).await,
        Err(StorageError::NotFound)
    ));
    let (first, second) = tokio::join!(
        store.enqueue_index(&owner.id, doc_id, &profile),
        reopened.enqueue_index(&owner.id, doc_id, &profile)
    );
    let first = first.unwrap();
    assert_eq!(first.id, second.unwrap().id);
    assert_eq!(first.status, "queued");
    assert_eq!(first.total_chunks, 17);
    let (a, b) = tokio::join!(store.claim_index(&profile), reopened.claim_index(&profile));
    let a = a.unwrap();
    let b = b.unwrap();
    assert_ne!(a.is_some(), b.is_some());
    let lease = a.or(b).unwrap();
    assert_eq!(lease.owner, owner.id);
    assert_eq!(
        store
            .enqueue_index(&owner.id, doc_id, &profile)
            .await
            .unwrap()
            .status,
        "running"
    );
    assert!(store.complete_index_batch(&lease, 17).await.is_err()); // 不允许跳过首批。
    store.complete_index_batch(&lease, 16).await.unwrap();
    let job = reopened
        .index_status(&owner.id, doc_id, &profile)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.indexed_chunks, 16);
    assert_eq!(job.status, "queued");
    let stale = reopened.claim_index(&profile).await.unwrap().unwrap();
    expire(&pool, &first.id).await;
    let recovered = store.claim_index(&profile).await.unwrap().unwrap();
    assert_eq!(recovered.job.indexed_chunks, 16);
    assert_eq!(recovered.job.attempts, 2);
    assert_ne!(recovered.token, stale.token);
    assert!(store.complete_index_batch(&stale, 17).await.is_err());
    assert!(
        store
            .fail_index_batch(&stale, IndexFailure::Timeout)
            .await
            .is_err()
    );
    store.complete_index_batch(&recovered, 17).await.unwrap();
    let done = store
        .enqueue_index(&owner.id, doc_id, &profile)
        .await
        .unwrap();
    assert_eq!(done.status, "succeeded");
    assert_eq!(done.indexed_chunks, 17);
    assert!(store.claim_index(&profile).await.unwrap().is_none());

    let retry_profile = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let retry = store
        .enqueue_index(&owner.id, doc_id, &retry_profile)
        .await
        .unwrap();
    assert!(store.claim_index(&profile).await.unwrap().is_none()); // 配置隔离。
    for attempt in 1..=5 {
        let lease = store.claim_index(&retry_profile).await.unwrap().unwrap();
        assert_eq!(lease.job.attempts, attempt);
        store
            .fail_index_batch(&lease, IndexFailure::ModelRateLimited)
            .await
            .unwrap();
        assert!(store.claim_index(&retry_profile).await.unwrap().is_none()); // 必须退避。
        ready(&pool, &retry.id).await;
    }
    assert!(store.claim_index(&retry_profile).await.unwrap().is_none());
    let failed = store
        .index_status(&owner.id, doc_id, &retry_profile)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.attempts, 5);
    let retried = store
        .enqueue_index(&owner.id, doc_id, &retry_profile)
        .await
        .unwrap();
    assert_eq!(retried.id, retry.id);
    assert_eq!(retried.attempts, 0);
    let lease = store.claim_index(&retry_profile).await.unwrap().unwrap();
    store.complete_index_batch(&lease, 16).await.unwrap();
    let lease = store.claim_index(&retry_profile).await.unwrap().unwrap();
    store
        .fail_index_batch(&lease, IndexFailure::InvalidEmbedding)
        .await
        .unwrap();
    assert_eq!(
        store
            .index_status(&owner.id, doc_id, &retry_profile)
            .await
            .unwrap()
            .unwrap()
            .status,
        "failed"
    );
    assert_eq!(
        store
            .enqueue_index(&owner.id, doc_id, &retry_profile)
            .await
            .unwrap()
            .indexed_chunks,
        16
    );
    let lease = store.claim_index(&retry_profile).await.unwrap().unwrap();
    store.complete_index_batch(&lease, 17).await.unwrap();

    let expired_profile = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let job = store
        .enqueue_index(&owner.id, doc_id, &expired_profile)
        .await
        .unwrap();
    for _ in 0..5 {
        assert!(store.claim_index(&expired_profile).await.unwrap().is_some());
        expire(&pool, &job.id).await;
    }
    assert!(store.claim_index(&expired_profile).await.unwrap().is_none());
    let expired = store
        .index_status(&owner.id, doc_id, &expired_profile)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expired.status, "failed");
    assert_eq!(expired.error_code.as_deref(), Some("lease_expired"));
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.id.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM document_index_jobs WHERE user_id=$1")
            .bind(Uuid::parse_str(owner.id.as_str()).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}
