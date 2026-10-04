use super::*;
use personal_ai_storage::{
    answer_authorizations::{
        AnswerApproval, AnswerAuthorization, AnswerAuthorizationStore, AnswerPreparation,
        SourceSelection,
    },
    documents::{DocumentStore, DocumentSummary, StoredDocument},
};
struct Fixture {
    store: PostgresStore,
    pool: sqlx::PgPool,
    owner: UserId,
    other: UserId,
    document: String,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let store = PostgresStore::connect(&url).await.unwrap();
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let owner = UserId::new(Uuid::new_v4().to_string());
        let other = UserId::new(Uuid::new_v4().to_string());
        for id in [&owner, &other] {
            store
                .save_user(&User {
                    id: id.clone(),
                    email: format!("{}@answer.example", id.as_str()),
                    display_name: "授权测试".into(),
                })
                .await
                .unwrap();
        }
        let document = StoredDocument {
            summary: DocumentSummary {
                id: Uuid::new_v4().to_string(),
                title: "私有材料".into(),
                source: "private.md".into(),
                source_type: "markdown".into(),
                tags: vec![],
                created_at_unix_ms: 1,
                chunk_count: 1,
            },
            markdown: "会议时间为周五。".into(),
            original_pdf: None,
            original_html: None,
            chunks: vec!["会议时间为周五。".into()],
        };
        store
            .insert_document(&owner, "answer-fixture", &document)
            .await
            .unwrap();
        Self {
            store,
            pool,
            owner,
            other,
            document: document.summary.id,
        }
    }
    fn input(&self) -> AnswerPreparation {
        AnswerPreparation {
            request_id: Uuid::new_v4().to_string(),
            question: "会议什么时候？".into(),
            endpoint: "http://127.0.0.1:11435".into(),
            model: "qwen3:4b-q4_K_M".into(),
            sources: vec![SourceSelection {
                document_id: self.document.clone(),
                ordinal: 0,
            }],
        }
    }
    async fn cleanup(&self) {
        sqlx::query("DELETE FROM users WHERE id=$1 OR id=$2")
            .bind(Uuid::parse_str(self.owner.as_str()).unwrap())
            .bind(Uuid::parse_str(self.other.as_str()).unwrap())
            .execute(&self.pool)
            .await
            .unwrap();
    }
}
fn approval(a: &AnswerAuthorization) -> AnswerApproval {
    AnswerApproval {
        digest: a.digest.clone(),
        acknowledge_sharing: true,
        acknowledge_local_compute: true,
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_authorizations_are_private_immutable_and_survive_reconnect() {
    let f = Fixture::new().await;
    let input = f.input();
    let (a, b) = tokio::join!(
        f.store.prepare_answer(&f.owner, &input),
        f.store.prepare_answer(&f.owner, &input)
    );
    let a = a.unwrap();
    assert_eq!(a.digest, b.unwrap().digest);
    assert_eq!(
        a.preview.as_ref().unwrap().materials[0].text,
        "会议时间为周五。"
    );
    let other = f
        .store
        .get_answer_authorization(&f.other, &input.request_id)
        .await;
    assert!(matches!(other, Err(StorageError::NotFound)));
    assert!(matches!(
        f.store
            .approve_answer(&f.other, &input.request_id, &approval(&a))
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store.cancel_answer(&f.other, &input.request_id).await,
        Err(StorageError::NotFound)
    ));
    assert!(
        f.store
            .list_answer_authorizations(&f.other)
            .await
            .unwrap()
            .is_empty()
    );
    let mut changed = input.clone();
    changed.question.push('!');
    assert!(matches!(
        f.store.prepare_answer(&f.owner, &changed).await,
        Err(StorageError::Conflict(_))
    ));
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let persisted = reopened
        .get_answer_authorization(&f.owner, &input.request_id)
        .await
        .unwrap();
    assert_eq!(persisted.digest, a.digest);
    assert_eq!(persisted.expires_at_unix_ms, a.expires_at_unix_ms);
    assert!(
        f.store
            .list_answer_authorizations(&f.owner)
            .await
            .unwrap()
            .iter()
            .all(|r| r.preview.is_none())
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_authorizations_require_consent_and_only_one_claim_commits() {
    let f = Fixture::new().await;
    let input = f.input();
    let a = f.store.prepare_answer(&f.owner, &input).await.unwrap();
    assert!(
        f.store
            .claim_answer(&f.owner, &input.request_id, &a.digest)
            .await
            .unwrap()
            .is_none()
    );
    for flags in [(false, true), (true, false)] {
        let mut approve = approval(&a);
        approve.acknowledge_sharing = flags.0;
        approve.acknowledge_local_compute = flags.1;
        assert!(matches!(
            f.store
                .approve_answer(&f.owner, &input.request_id, &approve)
                .await,
            Err(StorageError::InvalidData(_))
        ));
    }
    let mut wrong = approval(&a);
    wrong.digest = "0".repeat(64);
    assert!(matches!(
        f.store
            .approve_answer(&f.owner, &input.request_id, &wrong)
            .await,
        Err(StorageError::Conflict(_))
    ));
    for _ in 0..2 {
        f.store
            .approve_answer(&f.owner, &input.request_id, &approval(&a))
            .await
            .unwrap();
    }
    let (first, second) = tokio::join!(
        f.store.claim_answer(&f.owner, &input.request_id, &a.digest),
        f.store.claim_answer(&f.owner, &input.request_id, &a.digest)
    );
    let claims = [first.unwrap(), second.unwrap()];
    assert_eq!(claims.iter().filter(|c| c.is_some()).count(), 1);
    assert_eq!(
        claims.iter().flatten().next().unwrap().preview.question,
        input.question
    );
    let terminal = f.store.prepare_answer(&f.owner, &input).await.unwrap();
    assert_eq!(terminal.status, "consumed");
    assert!(terminal.preview.is_none());
    assert!(
        f.store
            .claim_answer(&f.owner, &input.request_id, &a.digest)
            .await
            .unwrap()
            .is_none()
    );
    let events: Vec<String> = sqlx::query_scalar(
        "SELECT event FROM answer_authorization_audit WHERE request_id=$1 ORDER BY event",
    )
    .bind(Uuid::parse_str(&input.request_id).unwrap())
    .fetch_all(&f.pool)
    .await
    .unwrap();
    assert_eq!(events, vec!["authorized", "consumed", "draft"]);
    let question: Option<String> =
        sqlx::query_scalar("SELECT question FROM answer_authorizations WHERE request_id=$1")
            .bind(Uuid::parse_str(&input.request_id).unwrap())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(question.is_none());
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_authorizations_invalidate_changed_sources_and_bindings() {
    for mutation in [
        "UPDATE documents SET chunks=ARRAY['changed'] WHERE id=$1",
        "UPDATE documents SET title='changed' WHERE id=$1",
        "UPDATE documents SET source='changed' WHERE id=$1",
        "DELETE FROM documents WHERE id=$1",
    ] {
        let f = Fixture::new().await;
        let input = f.input();
        let a = f.store.prepare_answer(&f.owner, &input).await.unwrap();
        f.store
            .approve_answer(&f.owner, &input.request_id, &approval(&a))
            .await
            .unwrap();
        sqlx::query(mutation)
            .bind(Uuid::parse_str(&f.document).unwrap())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            f.store
                .claim_answer(&f.owner, &input.request_id, &a.digest)
                .await
                .unwrap()
                .is_none()
        );
        let read = f
            .store
            .get_answer_authorization(&f.owner, &input.request_id)
            .await
            .unwrap();
        assert_eq!(read.status, "invalidated");
        assert!(read.preview.is_none());
        f.cleanup().await;
    }
    let f = Fixture::new().await;
    let input = f.input();
    f.store.prepare_answer(&f.owner, &input).await.unwrap();
    sqlx::query("UPDATE answer_authorizations SET sources=jsonb_set(sources,'{0,content_sha256}','\"tampered\"') WHERE request_id=$1").bind(Uuid::parse_str(&input.request_id).unwrap()).execute(&f.pool).await.unwrap();
    assert_eq!(
        f.store
            .get_answer_authorization(&f.owner, &input.request_id)
            .await
            .unwrap()
            .status,
        "invalidated"
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_authorizations_expire_cancel_and_roll_back_failed_audit() {
    let f = Fixture::new().await;
    let input = f.input();
    let a = f.store.prepare_answer(&f.owner, &input).await.unwrap();
    f.store
        .approve_answer(&f.owner, &input.request_id, &approval(&a))
        .await
        .unwrap();
    // A conflicting audit event simulates durable audit failure, scoped to this request only.
    sqlx::query("INSERT INTO answer_authorization_audit VALUES($1,$2,'consumed',1)")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .bind(Uuid::parse_str(&input.request_id).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(
        f.store
            .claim_answer(&f.owner, &input.request_id, &a.digest)
            .await
            .is_err()
    );
    assert_eq!(
        f.store
            .get_answer_authorization(&f.owner, &input.request_id)
            .await
            .unwrap()
            .status,
        "authorized"
    );
    assert_eq!(
        f.store
            .cancel_answer(&f.owner, &input.request_id)
            .await
            .unwrap()
            .status,
        "cancelled"
    );
    assert!(
        f.store
            .claim_answer(&f.owner, &input.request_id, &a.digest)
            .await
            .unwrap()
            .is_none()
    );
    let exp = f.input();
    let a = f.store.prepare_answer(&f.owner, &exp).await.unwrap();
    sqlx::query(
        "UPDATE answer_authorizations SET created_ms=1,expires_ms=600001 WHERE request_id=$1",
    )
    .bind(Uuid::parse_str(&exp.request_id).unwrap())
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        f.store
            .get_answer_authorization(&f.owner, &exp.request_id)
            .await
            .unwrap()
            .status,
        "expired"
    );
    assert!(matches!(
        f.store
            .approve_answer(&f.owner, &exp.request_id, &approval(&a))
            .await,
        Err(StorageError::Conflict(_))
    ));
    assert_eq!(
        f.store
            .prepare_answer(&f.owner, &exp)
            .await
            .unwrap()
            .expires_at_unix_ms,
        600_001
    );
    let cancel = f.input();
    f.store.prepare_answer(&f.owner, &cancel).await.unwrap();
    sqlx::query("DELETE FROM documents WHERE id=$1")
        .bind(Uuid::parse_str(&f.document).unwrap())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .cancel_answer(&f.owner, &cancel.request_id)
            .await
            .unwrap()
            .status,
        "cancelled"
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_authorizations_enforce_active_and_lifetime_quotas() {
    let f = Fixture::new().await;
    for _ in 0..20 {
        f.store.prepare_answer(&f.owner, &f.input()).await.unwrap();
    }
    assert!(matches!(
        f.store.prepare_answer(&f.owner, &f.input()).await,
        Err(StorageError::Conflict(_))
    ));
    let items = f.store.list_answer_authorizations(&f.owner).await.unwrap();
    assert_eq!(items.len(), 20);
    f.store
        .cancel_answer(&f.owner, &items[0].request_id)
        .await
        .unwrap();
    f.store.prepare_answer(&f.owner, &f.input()).await.unwrap();
    sqlx::query("INSERT INTO answer_authorizations SELECT user_id,gen_random_uuid(),endpoint,model,NULL,sources,intent_sha256,digest,'cancelled',created_ms,expires_ms FROM answer_authorizations CROSS JOIN generate_series(1,979) WHERE request_id=$1").bind(Uuid::parse_str(&items[0].request_id).unwrap()).execute(&f.pool).await.unwrap();
    f.store
        .cancel_answer(&f.owner, &items[1].request_id)
        .await
        .unwrap();
    assert!(matches!(
        f.store.prepare_answer(&f.owner, &f.input()).await,
        Err(StorageError::Conflict(_))
    ));
    assert_eq!(
        f.store
            .list_answer_authorizations(&f.owner)
            .await
            .unwrap()
            .len(),
        20
    );
    f.cleanup().await;
}
