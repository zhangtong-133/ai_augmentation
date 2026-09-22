use personal_ai_domain::{User, UserId};
use personal_ai_storage::{MetadataStore, StorageError};
use personal_ai_storage_postgres::PostgresStore;
use uuid::Uuid;

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn conversations_are_private_idempotent_and_deleted_without_resurrection() {
    use personal_ai_storage::conversations::ConversationStore;
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let owner = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@conversation.example", Uuid::new_v4()),
        display_name: "对话测试".into(),
    };
    store.save_user(&owner).await.unwrap();
    let foreign = UserId::new(Uuid::new_v4().to_string());
    let request = Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        store.create_conversation(&owner.id, &request, "学习"),
        store.create_conversation(&owner.id, &request, "学习")
    );
    let first = a.unwrap();
    assert_eq!(first, b.unwrap());
    assert_eq!(
        store.list_conversations(&owner.id).await.unwrap(),
        vec![first.clone()]
    );
    assert!(store.list_conversations(&foreign).await.unwrap().is_empty());
    assert!(matches!(
        store.get_conversation(&foreign, &first.id).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.delete_conversation(&foreign, &first.id).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store
            .create_conversation(&owner.id, &request, "不同标题")
            .await,
        Err(StorageError::Conflict(_))
    ));
    let reopened = PostgresStore::connect(&url).await.unwrap();
    assert_eq!(
        reopened
            .create_conversation(&owner.id, &request, "学习")
            .await
            .unwrap(),
        first
    );
    store
        .delete_conversation(&owner.id, &first.id)
        .await
        .unwrap();
    store
        .delete_conversation(&owner.id, &first.id)
        .await
        .unwrap();
    assert!(matches!(
        store.get_conversation(&owner.id, &first.id).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.create_conversation(&owner.id, &request, "学习").await,
        Err(StorageError::Conflict(_))
    ));
    assert!(
        store
            .list_conversations(&owner.id)
            .await
            .unwrap()
            .is_empty()
    );
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let title: String = sqlx::query_scalar("SELECT title FROM conversations WHERE id=$1")
        .bind(Uuid::parse_str(&first.id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "deleted");
    // 模拟超过保留期；下次创建时清理该用户的过期墓碑。
    sqlx::query("UPDATE conversations SET deleted_at=clock_timestamp()-interval '25 hours',created_at=clock_timestamp()-interval '26 hours' WHERE id=$1").bind(Uuid::parse_str(&first.id).unwrap()).execute(&pool).await.unwrap();
    let next = store
        .create_conversation(&owner.id, &request, "学习")
        .await
        .unwrap();
    assert_ne!(first.id, next.id);
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.id.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        store
            .list_conversations(&owner.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn conversation_quota_is_concurrent_durable_and_not_reset_by_delete() {
    use personal_ai_storage::conversations::ConversationStore;
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let owner = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@quota.example", Uuid::new_v4()),
        display_name: "配额测试".into(),
    };
    store.save_user(&owner).await.unwrap();
    for title in [" ", "\0", &"中".repeat(81)] {
        assert!(matches!(
            store
                .create_conversation(&owner.id, &Uuid::new_v4().to_string(), title)
                .await,
            Err(StorageError::InvalidData(_))
        ));
    }
    for _ in 0..99 {
        store
            .create_conversation(&owner.id, &Uuid::new_v4().to_string(), "对话")
            .await
            .unwrap();
    }
    let key_a = Uuid::new_v4().to_string();
    let key_b = Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        store.create_conversation(&owner.id, &key_a, "最后一个"),
        store.create_conversation(&owner.id, &key_b, "最后一个")
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(
        a.as_ref().err().or(b.as_ref().err()),
        Some(StorageError::Conflict(_))
    ));
    let records = store.list_conversations(&owner.id).await.unwrap();
    assert_eq!(records.len(), 100);
    for record in records {
        store
            .delete_conversation(&owner.id, &record.id)
            .await
            .unwrap();
    }
    let reopened = PostgresStore::connect(&url).await.unwrap();
    assert!(matches!(
        reopened
            .create_conversation(&owner.id, &Uuid::new_v4().to_string(), "删除不重置额度")
            .await,
        Err(StorageError::Conflict(_))
    ));
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.id.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing at a disposable PostgreSQL database"]
#[allow(clippy::too_many_lines)]
async fn long_memory_is_owner_scoped_versioned_bounded_and_durable() {
    use personal_ai_storage::long_memory::LongMemoryStore;
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let owner = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@memory.example", Uuid::new_v4()),
        display_name: "Memory".into(),
    };
    store.save_user(&owner).await.unwrap();
    let foreign = UserId::new(Uuid::new_v4().to_string());
    for (title, content) in [("", "x"), ("x", " "), ("x", "\0")] {
        assert!(matches!(
            store.create_fact(&owner.id, title, content).await,
            Err(StorageError::InvalidData(_))
        ));
    }
    let first = store
        .create_fact(&owner.id, "偏好", "中文回答")
        .await
        .unwrap();
    assert_eq!(first.version, 1);
    assert!(store.list_facts(&foreign, 0).await.unwrap().is_empty());
    assert!(matches!(
        store.update_fact(&foreign, &first.id, 1, "x", "y").await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        store.delete_fact(&foreign, &first.id, 1).await,
        Err(StorageError::NotFound)
    ));
    let (a, b) = tokio::join!(
        store.update_fact(&owner.id, &first.id, 1, "偏好", "A"),
        store.update_fact(&owner.id, &first.id, 1, "偏好", "B")
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(
        a.as_ref().err().or(b.as_ref().err()),
        Some(StorageError::Conflict(_))
    ));
    let reopened = PostgresStore::connect(&url).await.unwrap();
    let stored = reopened.list_facts(&owner.id, 0).await.unwrap();
    assert_eq!(stored[0].version, 2);
    assert_eq!(stored[0].created_at_unix_ms, first.created_at_unix_ms);
    assert!(matches!(
        store.delete_fact(&owner.id, &first.id, 1).await,
        Err(StorageError::Conflict(_))
    ));
    store.delete_fact(&owner.id, &first.id, 2).await.unwrap();
    assert!(matches!(
        store.delete_fact(&owner.id, &first.id, 2).await,
        Err(StorageError::NotFound)
    ));
    for index in 0..99 {
        store
            .create_fact(&owner.id, &format!("记忆{index}"), "内容")
            .await
            .unwrap();
    }
    let (a, b) = tokio::join!(
        store.create_fact(&owner.id, "最后一条", "A"),
        store.create_fact(&owner.id, "最后一条", "B")
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(
        a.as_ref().err().or(b.as_ref().err()),
        Some(StorageError::Conflict(_))
    ));
    let mut ids = std::collections::HashSet::new();
    for offset in [0, 20, 40, 60, 80] {
        for entry in store.list_facts(&owner.id, offset).await.unwrap() {
            assert!(ids.insert(entry.id));
        }
    }
    assert_eq!(ids.len(), 100);
    assert!(store.list_facts(&owner.id, 100).await.unwrap().is_empty());
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.id.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.list_facts(&owner.id, 0).await.unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing at a disposable PostgreSQL database"]
#[allow(clippy::too_many_lines)] // 同一生命周期覆盖并发领取、故障恢复、重试上限与隔离。
async fn index_jobs_are_durable_fenced_bounded_and_owner_scoped() {
    use personal_ai_storage::{
        documents::{DocumentStore, DocumentSummary, StoredDocument},
        index_jobs::{BatchOutcome, IndexJobStore},
    };
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let store = PostgresStore::connect(&url).await.unwrap();
    let owner = User {
        id: UserId::new(Uuid::new_v4().to_string()),
        email: format!("{}@example.com", Uuid::new_v4()),
        display_name: "Jobs".into(),
    };
    store.save_user(&owner).await.unwrap();
    let document = StoredDocument {
        summary: DocumentSummary {
            id: Uuid::new_v4().to_string(),
            title: "队列测试".into(),
            source: "test.md".into(),
            source_type: "markdown".into(),
            tags: vec![],
            created_at_unix_ms: 1,
            chunk_count: 17,
        },
        markdown: "test".into(),
        original_pdf: None,
        original_html: None,
        chunks: vec!["test".into(); 17],
    };
    store
        .insert_document(&owner.id, "jobs", &document)
        .await
        .unwrap();
    let doc = &document.summary.id;
    let target = Uuid::new_v4().to_string();
    let foreign = UserId::new(Uuid::new_v4().to_string());
    assert!(matches!(
        store.enqueue(&foreign, doc, &target).await,
        Err(StorageError::NotFound)
    ));
    let (a, b) = tokio::join!(
        store.enqueue(&owner.id, doc, &target),
        store.enqueue(&owner.id, doc, &target)
    );
    assert_eq!(a.unwrap().status, "queued");
    assert_eq!(b.unwrap().status, "queued");
    assert!(matches!(
        store.index_status(&foreign, doc, &target).await,
        Err(StorageError::NotFound)
    ));
    assert!(store.claim("different-target").await.unwrap().is_none());
    let reopened = PostgresStore::connect(&url).await.unwrap();
    let (a, b) = tokio::join!(store.claim(&target), reopened.claim(&target));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.is_some(), b.is_some());
    let first = a.or(b).unwrap();
    assert_eq!(first.attempts, 1);
    let duplicate = store.enqueue(&owner.id, doc, &target).await.unwrap();
    assert_eq!(duplicate.lease, first.lease);
    assert_eq!(duplicate.attempts, 1);
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    // 模拟进程崩溃后的租约过期，不使用真实一分钟等待。
    sqlx::query(
        "UPDATE document_index_jobs SET lease_until=NOW()-INTERVAL '1 second' WHERE document_id=$1",
    )
    .bind(Uuid::parse_str(doc).unwrap())
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        store
            .finish_batch(&target, &first, BatchOutcome::Success(16))
            .await,
        Err(StorageError::Conflict(_))
    ));
    let second = reopened.claim(&target).await.unwrap().unwrap();
    assert_ne!(second.lease, first.lease);
    assert_eq!(second.attempts, 2);
    assert!(
        store
            .finish_batch(&target, &first, BatchOutcome::Success(16))
            .await
            .is_err()
    );
    store
        .finish_batch(&target, &second, BatchOutcome::Success(16))
        .await
        .unwrap();
    let tail = store.claim(&target).await.unwrap().unwrap();
    assert_eq!(tail.indexed_chunks, 16);
    assert_eq!(tail.attempts, 1);
    store
        .finish_batch(&target, &tail, BatchOutcome::Retry("embedding_unavailable"))
        .await
        .unwrap();
    assert!(store.claim(&target).await.unwrap().is_none());
    assert_eq!(
        store
            .index_status(&owner.id, doc, &target)
            .await
            .unwrap()
            .status,
        "retrying"
    );
    for attempt in [2, 3] {
        sqlx::query("UPDATE document_index_jobs SET available_at=NOW()-INTERVAL '1 second' WHERE document_id=$1").bind(Uuid::parse_str(doc).unwrap()).execute(&pool).await.unwrap();
        let retry = store.claim(&target).await.unwrap().unwrap();
        assert_eq!(retry.attempts, attempt);
        assert_eq!(retry.indexed_chunks, 16);
        store
            .finish_batch(
                &target,
                &retry,
                BatchOutcome::Retry("embedding_unavailable"),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        store
            .index_status(&owner.id, doc, &target)
            .await
            .unwrap()
            .status,
        "failed"
    );
    assert!(store.claim(&target).await.unwrap().is_none());
    let manual = store.enqueue(&owner.id, doc, &target).await.unwrap();
    assert_eq!(manual.indexed_chunks, 16);
    assert_eq!(manual.attempts, 0);
    assert!(manual.error_code.is_none());
    let last = store.claim(&target).await.unwrap().unwrap();
    store
        .finish_batch(&target, &last, BatchOutcome::Success(17))
        .await
        .unwrap();
    assert_eq!(
        reopened
            .index_status(&owner.id, doc, &target)
            .await
            .unwrap()
            .status,
        "completed"
    );
    assert_eq!(
        store.enqueue(&owner.id, doc, &target).await.unwrap().status,
        "completed"
    );
    assert!(store.claim(&target).await.unwrap().is_none());
    let next_target = format!("{target}-new");
    assert_eq!(
        store
            .enqueue(&owner.id, doc, &next_target)
            .await
            .unwrap()
            .indexed_chunks,
        0
    );
    for attempt in 1..=3 {
        let crashed = store.claim(&next_target).await.unwrap().unwrap();
        assert_eq!(crashed.attempts, attempt);
        sqlx::query("UPDATE document_index_jobs SET lease_until=NOW()-INTERVAL '1 second' WHERE document_id=$1").bind(Uuid::parse_str(doc).unwrap()).execute(&pool).await.unwrap();
    }
    assert!(store.claim(&next_target).await.unwrap().is_none());
    assert_eq!(
        store
            .index_status(&owner.id, doc, &next_target)
            .await
            .unwrap()
            .error_code
            .as_deref(),
        Some("lease_expired")
    );
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(Uuid::parse_str(owner.id.as_str()).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        store.index_status(&owner.id, doc, &target).await,
        Err(StorageError::NotFound)
    ));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document_index_jobs WHERE document_id=$1")
            .bind(Uuid::parse_str(doc).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

// 需要可丢弃的 PostgreSQL 数据库，CI 会提供专用服务。
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
    // 重新连接并再次执行迁移，验证重启安全性和数据持久化。
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
#[allow(clippy::too_many_lines)] // 在同一测试中覆盖持久化、用户隔离及聚合统计边界。
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
            source_type: "markdown".into(),
            tags: vec!["Rust".into()],
            created_at_unix_ms: 1234,
            chunk_count: 1,
        },
        markdown: "# 中文".into(),
        original_pdf: None,
        original_html: None,
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
    let pdf = StoredDocument {
        summary: DocumentSummary {
            id: Uuid::new_v4().to_string(),
            source_type: "pdf".into(),
            source: "note.pdf".into(),
            ..second.summary.clone()
        },
        markdown: "Extracted text **literal**".into(),
        chunks: vec!["Extracted text **literal**".into()],
        original_pdf: Some(b"%PDF-test-original".to_vec()),
        original_html: None,
    };
    reopened
        .insert_document(&owner.id, "pdf:test-digest", &pdf)
        .await
        .unwrap();
    let again = PostgresStore::connect(&url).await.unwrap();
    assert_eq!(
        again
            .get_document(&owner.id, &pdf.summary.id)
            .await
            .unwrap(),
        pdf
    );
    assert!(matches!(
        again.get_document(&other.id, &pdf.summary.id).await,
        Err(StorageError::NotFound)
    ));
    let web = StoredDocument {
        summary: DocumentSummary {
            id: Uuid::new_v4().to_string(),
            source_type: "web_page".into(),
            source: "https://public.example/article".into(),
            ..pdf.summary.clone()
        },
        markdown: "网页正文".into(),
        chunks: vec!["网页正文".into()],
        original_pdf: None,
        original_html: Some("<article>网页正文<script>untrusted()</script></article>".into()),
    };
    again
        .insert_document(&owner.id, "web:test-digest", &web)
        .await
        .unwrap();
    let reopened_web = PostgresStore::connect(&url).await.unwrap();
    assert_eq!(
        reopened_web
            .get_document(&owner.id, &web.summary.id)
            .await
            .unwrap(),
        web
    );
    assert!(matches!(
        reopened_web.get_document(&other.id, &web.summary.id).await,
        Err(StorageError::NotFound)
    ));
}
