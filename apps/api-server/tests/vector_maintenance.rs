use personal_ai_domain::{User, UserId};
use personal_ai_knowledge::vector_maintenance::collect_vectors;
use personal_ai_storage::{
    BoxFuture, EmbeddingRecord, MetadataStore, StorageError, StorageResult, VectorStore,
    documents::{DocumentStore, DocumentSummary, StoredDocument},
    vector_maintenance::{DocumentPresenceStore, VectorMaintenanceStore},
};
use personal_ai_storage_postgres::PostgresStore;
use personal_ai_storage_qdrant::QdrantStore;
use std::process::Command;
use uuid::Uuid;

struct Unavailable;
impl DocumentPresenceStore for Unavailable {
    fn document_exists(&self, _: &UserId, _: &str) -> BoxFuture<'_, StorageResult<bool>> {
        Box::pin(async { Err(StorageError::Unavailable("metadata offline".into())) })
    }
}
fn cli(
    db: &str,
    qdrant: &str,
    collection: &str,
    owner: &UserId,
    after: Option<&str>,
    apply: bool,
) -> serde_json::Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vector-maintenance"));
    command
        .args(["gc", "--user", owner.as_str()])
        .env("DATABASE_URL", db)
        .env("QDRANT_URL", qdrant)
        .env("QDRANT_COLLECTION", collection)
        .env("OPENAI_EMBEDDING_MODEL", "m")
        .env("EMBEDDING_DIMENSIONS", "3")
        .env_remove("QDRANT_API_KEY");
    if let Some(after) = after {
        command.args(["--after", after]);
    }
    if apply {
        command.args(["--apply", "--exclusive-collection", "--writers-stopped"]);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("private-text"));
    assert!(!text.contains("document_id"));
    serde_json::from_str(&text).unwrap()
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL/Qdrant; run make smoke-index"]
#[allow(clippy::too_many_lines)] // 真实 CLI 分页预览、执行、重复运行及删除用户后的闭环。
async fn cleanup_preserves_live_documents_and_other_owners_models_and_handles_deleted_users() {
    let db = std::env::var("TEST_DATABASE_URL").unwrap();
    let url = std::env::var("TEST_QDRANT_URL").unwrap();
    let store = PostgresStore::connect(&db).await.unwrap();
    let pool = sqlx::PgPool::connect(&db).await.unwrap();
    let owner = UserId::new(Uuid::new_v4().to_string());
    let other = UserId::new(Uuid::new_v4().to_string());
    store
        .save_user(&User {
            id: owner.clone(),
            email: format!("{}@vectors.example", owner.as_str()),
            display_name: "gc".into(),
        })
        .await
        .unwrap();
    let doc = StoredDocument {
        summary: DocumentSummary {
            id: Uuid::new_v4().to_string(),
            title: "private-text".into(),
            source: "test.md".into(),
            source_type: "markdown".into(),
            tags: vec![],
            created_at_unix_ms: 1,
            chunk_count: 1,
        },
        markdown: "private-text".into(),
        original_pdf: None,
        original_html: None,
        chunks: vec!["private-text".into()],
    };
    store.insert_document(&owner, "gc", &doc).await.unwrap();
    assert!(
        store
            .document_exists(&owner, &doc.summary.id)
            .await
            .unwrap()
    );
    assert!(
        !store
            .document_exists(&other, &doc.summary.id)
            .await
            .unwrap()
    );
    let collection = format!("gc_{}", Uuid::new_v4().simple());
    let vectors = QdrantStore::new(&url, &collection, None, "m", 3).unwrap();
    vectors.ensure_collection().await.unwrap();
    let model = QdrantStore::new(&url, &collection, None, "other", 3).unwrap();
    let live = EmbeddingRecord {
        id: "live".into(),
        document_id: doc.summary.id.clone(),
        ordinal: 0,
        text: "private-text".into(),
        model: "m".into(),
        vector: vec![1.0, 0.0, 0.0],
        source: "test.md".into(),
        content_type: "markdown".into(),
        created_at_unix_ms: 1,
        tags: vec![],
    };
    vectors
        .insert_embeddings(&owner, std::slice::from_ref(&live))
        .await
        .unwrap();
    vectors
        .insert_embeddings(&other, std::slice::from_ref(&live))
        .await
        .unwrap();
    model
        .insert_embeddings(
            &owner,
            &[EmbeddingRecord {
                model: "other".into(),
                ..live.clone()
            }],
        )
        .await
        .unwrap();
    let orphan_doc = Uuid::new_v4().to_string();
    let records: Vec<_> = (0..101)
        .map(|n| EmbeddingRecord {
            id: format!("orphan-{n}"),
            document_id: orphan_doc.clone(),
            ..live.clone()
        })
        .collect();
    for chunk in records.chunks(16) {
        vectors.insert_embeddings(&owner, chunk).await.unwrap();
    }
    assert!(
        collect_vectors(&Unavailable, &vectors, &owner, None, true)
            .await
            .is_err()
    );
    let preview = cli(&db, &url, &collection, &owner, None, false);
    assert_eq!(preview["scanned"], 100);
    assert_eq!(preview["changed"], 0);
    assert!(preview["next_cursor"].is_string());
    assert_eq!(
        vectors
            .list_references(&owner, None)
            .await
            .unwrap()
            .items
            .len(),
        100
    );
    let mut cursor = None;
    let mut removed = 0;
    for _ in 0..3 {
        let report = cli(&db, &url, &collection, &owner, cursor.as_deref(), true);
        removed += report["changed"].as_u64().unwrap();
        cursor = report["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert!(cursor.is_none());
    assert_eq!(removed, 101);
    let remaining = vectors.list_references(&owner, None).await.unwrap();
    assert_eq!(remaining.items.len(), 1);
    assert_eq!(remaining.items[0].id, "live");
    assert_eq!(
        cli(&db, &url, &collection, &owner, None, true)["changed"],
        0
    );
    assert_eq!(
        vectors
            .list_references(&other, None)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        model
            .list_references(&owner, None)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        cli(&db, &url, &collection, &owner, None, true)["changed"],
        1
    );
    assert_eq!(
        vectors
            .list_references(&owner, None)
            .await
            .unwrap()
            .items
            .len(),
        0
    );
    assert_eq!(
        model
            .list_references(&owner, None)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}
