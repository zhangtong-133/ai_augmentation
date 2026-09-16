use personal_ai_domain::{User, UserId};
use personal_ai_storage::{
    BoxFuture, MetadataStore, ObjectStorage, StorageError, StorageResult,
    documents::{DocumentStore, DocumentSummary, StoredDocument},
};
use personal_ai_storage_postgres::PostgresStore;
use personal_ai_storage_s3::S3Store;
use sqlx::Row;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use uuid::Uuid;

struct FaultStore {
    inner: S3Store,
    fail_put: AtomicBool,
    reads: AtomicUsize,
    writes: AtomicUsize,
}
impl ObjectStorage for FaultStore {
    fn put(
        &self,
        key: &str,
        bytes: &[u8],
        content_type: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<()>> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        if self.fail_put.load(Ordering::SeqCst) {
            Box::pin(async { Err(StorageError::Unavailable("injected failure".into())) })
        } else {
            self.inner.put(key, bytes, content_type)
        }
    }
    fn get(&self, key: &str) -> BoxFuture<'_, StorageResult<Vec<u8>>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.get(key)
    }
    fn delete(&self, key: &str) -> BoxFuture<'_, StorageResult<()>> {
        self.inner.delete(key)
    }
}
fn document(source_type: &str) -> StoredDocument {
    StoredDocument {
        summary: DocumentSummary {
            id: Uuid::new_v4().to_string(),
            title: "原文验证".into(),
            source: "test".into(),
            source_type: source_type.into(),
            tags: vec![],
            created_at_unix_ms: 1234,
            chunk_count: 1,
        },
        markdown: "# 中文原文\r\n".into(),
        chunks: vec!["中文原文".into()],
        original_pdf: (source_type == "pdf").then(|| b"%PDF-original\x00\xff".to_vec()),
        original_html: (source_type == "web_page")
            .then(|| "<html><body>中文原文</body></html>".into()),
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and MinIO; run make smoke-objects"]
#[allow(clippy::too_many_lines)]
async fn originals_roundtrip_isolation_failure_and_legacy_compatibility() {
    let env = |key| std::env::var(key).expect("test configuration required");
    let url = env("TEST_DATABASE_URL");
    let objects = Arc::new(FaultStore {
        inner: S3Store::new(
            &env("TEST_OBJECT_ENDPOINT"),
            &env("TEST_OBJECT_BUCKET"),
            "us-east-1",
            &env("TEST_OBJECT_ACCESS_KEY"),
            &env("TEST_OBJECT_SECRET_KEY"),
        )
        .unwrap(),
        fail_put: AtomicBool::new(false),
        reads: AtomicUsize::new(0),
        writes: AtomicUsize::new(0),
    });
    let legacy = PostgresStore::connect(&url).await.unwrap();
    let store = PostgresStore::connect(&url)
        .await
        .unwrap()
        .with_object_storage(objects.clone());
    let user = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@example.com", Uuid::new_v4()),
        display_name: "Owner".into(),
    };
    store.save_user(&user).await.unwrap();
    let other = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@example.com", Uuid::new_v4()),
        display_name: "Other".into(),
    };
    store.save_user(&other).await.unwrap();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let missing_bucket = Arc::new(
        S3Store::new(
            &env("TEST_OBJECT_ENDPOINT"),
            &format!("missing-{}", Uuid::new_v4()),
            "us-east-1",
            &env("TEST_OBJECT_ACCESS_KEY"),
            &env("TEST_OBJECT_SECRET_KEY"),
        )
        .unwrap(),
    );
    let broken = PostgresStore::connect(&url)
        .await
        .unwrap()
        .with_object_storage(missing_bucket);
    let rejected = document("markdown");
    assert!(matches!(
        broken
            .insert_document(&user.id, "missing-bucket", &rejected)
            .await,
        Err(StorageError::Unavailable(_))
    ));
    assert!(matches!(
        store.get_document(&user.id, &rejected.summary.id).await,
        Err(StorageError::NotFound)
    ));

    for kind in ["markdown", "pdf", "web_page"] {
        let old = document(kind);
        legacy
            .insert_document(&user.id, &format!("legacy-{kind}"), &old)
            .await
            .unwrap();
        assert_eq!(
            store.get_document(&user.id, &old.summary.id).await.unwrap(),
            old
        );
        let doc = document(kind);
        store.insert_document(&user.id, kind, &doc).await.unwrap();
        let row = sqlx::query(
            "SELECT original_object_key,original_pdf,original_html FROM documents WHERE id=$1",
        )
        .bind(Uuid::parse_str(&doc.summary.id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        let key: String = row.get("original_object_key");
        assert!(key.starts_with(&format!(
            "users/{}/documents/{}/",
            user.id.as_str(),
            doc.summary.id
        )));
        assert!(row.get::<Option<Vec<u8>>, _>("original_pdf").is_none());
        assert!(row.get::<Option<String>, _>("original_html").is_none());
        let expected = match kind {
            "pdf" => doc.original_pdf.as_ref().unwrap().clone(),
            "web_page" => doc.original_html.as_ref().unwrap().as_bytes().to_vec(),
            _ => doc.markdown.as_bytes().to_vec(),
        };
        assert_eq!(objects.get(&key).await.unwrap(), expected);
        let reopened = PostgresStore::connect(&url)
            .await
            .unwrap()
            .with_object_storage(objects.clone());
        assert_eq!(
            reopened
                .get_document(&user.id, &doc.summary.id)
                .await
                .unwrap(),
            doc
        );
        let reads = objects.reads.load(Ordering::SeqCst);
        assert!(matches!(
            store.get_document(&other.id, &doc.summary.id).await,
            Err(StorageError::NotFound)
        ));
        assert_eq!(objects.reads.load(Ordering::SeqCst), reads);
        let writes = objects.writes.load(Ordering::SeqCst);
        assert!(matches!(
            store.insert_document(&user.id, kind, &document(kind)).await,
            Err(StorageError::Conflict(_))
        ));
        assert_eq!(objects.writes.load(Ordering::SeqCst), writes);
        assert!(matches!(
            legacy.get_document(&user.id, &doc.summary.id).await,
            Err(StorageError::Unavailable(_))
        ));
        assert!(!legacy.list_documents(&user.id, 0).await.unwrap().is_empty());
        // 相同原文可由另一用户独立导入。
        let other_doc = document(kind);
        store
            .insert_document(&other.id, kind, &other_doc)
            .await
            .unwrap();
        assert_eq!(
            store
                .get_document(&other.id, &other_doc.summary.id)
                .await
                .unwrap(),
            other_doc
        );
        objects.delete(&key).await.unwrap();
        assert!(matches!(
            objects.get(&key).await,
            Err(StorageError::NotFound)
        ));
        assert!(matches!(
            store.get_document(&user.id, &doc.summary.id).await,
            Err(StorageError::Unavailable(_))
        ));
    }
    objects.fail_put.store(true, Ordering::SeqCst);
    let failed = document("markdown");
    assert!(matches!(
        store.insert_document(&user.id, "retry", &failed).await,
        Err(StorageError::Unavailable(_))
    ));
    assert!(matches!(
        store.get_document(&user.id, &failed.summary.id).await,
        Err(StorageError::NotFound)
    ));
    objects.fail_put.store(false, Ordering::SeqCst);
    store
        .insert_document(&user.id, "retry", &failed)
        .await
        .unwrap();
    assert_eq!(
        store
            .get_document(&user.id, &failed.summary.id)
            .await
            .unwrap(),
        failed
    );
}
