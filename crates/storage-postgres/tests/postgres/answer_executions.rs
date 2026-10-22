use super::*;
use personal_ai_storage::{
    answer_authorizations::{
        AnswerApproval, AnswerAuthorization, AnswerAuthorizationStore, AnswerPreparation,
        SourceSelection,
    },
    answer_executions::{
        AnswerExecutionClaim, AnswerExecutionOutcome, AnswerExecutionStore, StoredAnswerCitation,
        StoredAnswerResult,
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
                    email: format!("{}@answer-execution.example", id.as_str()),
                    display_name: "执行测试".into(),
                })
                .await
                .unwrap();
        }
        let document = StoredDocument {
            summary: DocumentSummary {
                id: Uuid::new_v4().to_string(),
                title: "执行私有材料".into(),
                source: "execution-private.md".into(),
                source_type: "markdown".into(),
                tags: vec![],
                created_at_unix_ms: 1,
                chunk_count: 2,
            },
            markdown: "会议时间为周五。会议地点为西厅。".into(),
            original_pdf: None,
            original_html: None,
            chunks: vec!["会议时间为周五。".into(), "会议地点为西厅。".into()],
        };
        store
            .insert_document(&owner, "answer-execution-fixture", &document)
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

    async fn authorize(&self, input: &AnswerPreparation) -> AnswerAuthorization {
        let item = self.store.prepare_answer(&self.owner, input).await.unwrap();
        self.store
            .approve_answer(
                &self.owner,
                &input.request_id,
                &AnswerApproval {
                    digest: item.digest.clone(),
                    acknowledge_sharing: true,
                    acknowledge_local_compute: true,
                },
            )
            .await
            .unwrap()
    }

    async fn claim(&self, input: &AnswerPreparation) -> AnswerExecutionClaim {
        let item = self.authorize(input).await;
        self.store
            .claim_answer_execution(&self.owner, &input.request_id, &item.digest)
            .await
            .unwrap()
            .unwrap()
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

fn result() -> StoredAnswerResult {
    StoredAnswerResult {
        answer: "会议在周五。".into(),
        citations: vec![StoredAnswerCitation {
            id: 1,
            quote: "会议时间为周五。".into(),
        }],
        insufficient_evidence: false,
    }
}

fn copy_claim(claim: &AnswerExecutionClaim) -> AnswerExecutionClaim {
    AnswerExecutionClaim {
        owner: claim.owner.clone(),
        request_id: claim.request_id.clone(),
        token: claim.token.clone(),
        digest: claim.digest.clone(),
        preview: claim.preview.clone(),
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_execution_claim_and_sending_are_once_only_and_survive_reconnect() {
    let f = Fixture::new().await;
    let input = f.input();
    let authorization = f.authorize(&input).await;
    let (first, second) = tokio::join!(
        f.store
            .claim_answer_execution(&f.owner, &input.request_id, &authorization.digest),
        f.store
            .claim_answer_execution(&f.owner, &input.request_id, &authorization.digest)
    );
    let claims = [first.unwrap(), second.unwrap()];
    assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
    let claim = claims.into_iter().flatten().next().unwrap();
    let execution = f
        .store
        .get_answer_execution(&f.owner, &input.request_id)
        .await
        .unwrap();
    assert_eq!(execution.status, "running");
    assert_eq!(
        execution.deadline_at_unix_ms - execution.started_at_unix_ms,
        90_000
    );
    assert!(execution.deadline_at_unix_ms <= authorization.expires_at_unix_ms);
    assert_eq!(
        execution.preview.unwrap().request_sha256,
        claim.preview.request_sha256
    );
    let consumed = f
        .store
        .get_answer_authorization(&f.owner, &input.request_id)
        .await
        .unwrap();
    assert_eq!(consumed.status, "consumed");
    assert!(consumed.preview.is_none());
    assert!(
        f.store
            .claim_answer(&f.owner, &input.request_id, &claim.digest)
            .await
            .unwrap()
            .is_none()
    );
    let (a, b) = tokio::join!(
        f.store.begin_answer_execution(&claim),
        f.store.begin_answer_execution(&claim)
    );
    assert_eq!(
        [a.unwrap(), b.unwrap()]
            .into_iter()
            .filter(|value| *value)
            .count(),
        1
    );
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert!(!reopened.begin_answer_execution(&claim).await.unwrap());
    assert!(reopened.check_answer_execution(&claim).await.unwrap());
    assert!(
        reopened
            .claim_answer_execution(&f.owner, &input.request_id, &claim.digest)
            .await
            .unwrap()
            .is_none()
    );
    let events: Vec<String> = sqlx::query_scalar("SELECT event FROM answer_execution_audit WHERE user_id=$1 AND request_id=$2 ORDER BY event")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&input.request_id).unwrap())
        .fetch_all(&f.pool).await.unwrap();
    assert_eq!(events, vec!["running", "sending"]);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_execution_results_are_private_persistent_and_absent_from_lists() {
    let f = Fixture::new().await;
    let input = f.input();
    assert!(matches!(
        f.store
            .get_answer_execution(&f.owner, &input.request_id)
            .await,
        Err(StorageError::NotFound)
    ));
    let claim = f.claim(&input).await;
    for attempt in [
        f.store
            .get_answer_execution(&f.other, &input.request_id)
            .await,
        f.store
            .cancel_answer_execution(&f.other, &input.request_id)
            .await,
    ] {
        assert!(matches!(attempt, Err(StorageError::NotFound)));
    }
    assert!(
        f.store
            .list_answer_executions(&f.other)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        f.store
            .claim_answer_execution(&f.other, &input.request_id, &claim.digest)
            .await,
        Err(StorageError::NotFound)
    ));
    let mut foreign = copy_claim(&claim);
    foreign.owner = f.other.clone();
    assert!(matches!(
        f.store.begin_answer_execution(&foreign).await,
        Err(StorageError::NotFound)
    ));
    assert!(matches!(
        f.store
            .finish_answer_execution(&foreign, AnswerExecutionOutcome::Unknown)
            .await,
        Err(StorageError::NotFound)
    ));
    assert!(f.store.begin_answer_execution(&claim).await.unwrap());
    let succeeded = f
        .store
        .finish_answer_execution(&claim, AnswerExecutionOutcome::Succeeded(result()))
        .await
        .unwrap();
    assert_eq!(succeeded.status, "succeeded");
    assert_eq!(succeeded.result, Some(result()));
    assert_eq!(
        succeeded.preview.unwrap().materials[0].text,
        "会议时间为周五。"
    );
    let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let saved = reopened
        .get_answer_execution(&f.owner, &input.request_id)
        .await
        .unwrap();
    assert_eq!(saved.result, Some(result()));
    let list = reopened.list_answer_executions(&f.owner).await.unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].preview.is_none() && list[0].result.is_none());
    let serialized = serde_json::to_string(&list).unwrap();
    for private in [
        "会议",
        "周五",
        "execution-private.md",
        "执行私有材料",
        "dispatch_token",
    ] {
        assert!(!serialized.contains(private));
    }
    let stored: serde_json::Value = sqlx::query_scalar(
        "SELECT result FROM answer_executions WHERE user_id=$1 AND request_id=$2",
    )
    .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
    .bind(Uuid::parse_str(&input.request_id).unwrap())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(stored, serde_json::to_value(result()).unwrap());
    assert!(!stored.to_string().contains("西厅"));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_execution_invalidates_changed_sources_before_sending_or_saving() {
    for mutation in [
        "UPDATE documents SET chunks=ARRAY['changed'] WHERE id=$1",
        "UPDATE documents SET title='changed' WHERE id=$1",
        "UPDATE documents SET source='changed' WHERE id=$1",
        "DELETE FROM documents WHERE id=$1",
    ] {
        for sent in [false, true] {
            let f = Fixture::new().await;
            let input = f.input();
            let claim = f.claim(&input).await;
            if sent {
                assert!(f.store.begin_answer_execution(&claim).await.unwrap());
            }
            sqlx::query(mutation)
                .bind(Uuid::parse_str(&f.document).unwrap())
                .execute(&f.pool)
                .await
                .unwrap();
            assert!(!f.store.check_answer_execution(&claim).await.unwrap());
            assert!(!f.store.begin_answer_execution(&claim).await.unwrap());
            let saved = f
                .store
                .finish_answer_execution(&claim, AnswerExecutionOutcome::Succeeded(result()))
                .await
                .unwrap();
            assert_eq!(saved.status, "invalidated");
            assert!(saved.preview.is_none() && saved.result.is_none());
            let body: Option<String> = sqlx::query_scalar(
                "SELECT question FROM answer_executions WHERE user_id=$1 AND request_id=$2",
            )
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
            .bind(Uuid::parse_str(&input.request_id).unwrap())
            .fetch_one(&f.pool)
            .await
            .unwrap();
            assert!(body.is_none());
            assert!(
                f.store
                    .claim_answer_execution(&f.owner, &input.request_id, &claim.digest)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                f.store
                    .get_answer_authorization(&f.owner, &input.request_id)
                    .await
                    .unwrap()
                    .status,
                "consumed"
            );
            f.cleanup().await;
        }
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_execution_completed_reads_recheck_all_materials_and_clear_stale_results() {
    for mutation in [
        "UPDATE documents SET chunks[2]='changed uncited chunk' WHERE id=$1",
        "UPDATE documents SET title='changed' WHERE id=$1",
        "UPDATE documents SET source='changed' WHERE id=$1",
        "DELETE FROM documents WHERE id=$1",
    ] {
        let f = Fixture::new().await;
        let mut input = f.input();
        input.sources.push(SourceSelection {
            document_id: f.document.clone(),
            ordinal: 1,
        });
        let claim = f.claim(&input).await;
        assert!(f.store.begin_answer_execution(&claim).await.unwrap());
        assert_eq!(
            f.store
                .finish_answer_execution(&claim, AnswerExecutionOutcome::Succeeded(result()))
                .await
                .unwrap()
                .status,
            "succeeded"
        );
        sqlx::query(mutation)
            .bind(Uuid::parse_str(&f.document).unwrap())
            .execute(&f.pool)
            .await
            .unwrap();
        let saved = f
            .store
            .get_answer_execution(&f.owner, &input.request_id)
            .await
            .unwrap();
        assert_eq!(saved.status, "invalidated");
        assert!(saved.result.is_none() && saved.preview.is_none());
        let bodies: (Option<String>, Option<serde_json::Value>) = sqlx::query_as(
            "SELECT question,result FROM answer_executions WHERE user_id=$1 AND request_id=$2",
        )
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap())
        .bind(Uuid::parse_str(&input.request_id).unwrap())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(bodies, (None, None));
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_execution_rechecks_authorization_and_request_fingerprints() {
    for mutation in [
        "UPDATE answer_authorizations SET sources=jsonb_set(sources,'{0,content_sha256}','\"tampered\"') WHERE request_id=$1",
        "UPDATE answer_authorizations SET intent_sha256=repeat('0',64) WHERE request_id=$1",
        "UPDATE answer_authorizations SET digest=repeat('0',64) WHERE request_id=$1",
        "UPDATE answer_authorizations SET model='different' WHERE request_id=$1",
        "UPDATE answer_executions SET request_sha256=repeat('0',64) WHERE request_id=$1",
    ] {
        let f = Fixture::new().await;
        let input = f.input();
        f.claim(&input).await;
        sqlx::query(mutation)
            .bind(Uuid::parse_str(&input.request_id).unwrap())
            .execute(&f.pool)
            .await
            .unwrap();
        let item = f
            .store
            .get_answer_execution(&f.owner, &input.request_id)
            .await
            .unwrap();
        assert_eq!(item.status, "invalidated");
        assert!(item.preview.is_none() && item.result.is_none());
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_execution_expired_running_becomes_unknown_and_cannot_be_resent() {
    for sent in [false, true] {
        let f = Fixture::new().await;
        let input = f.input();
        let claim = f.claim(&input).await;
        if sent {
            assert!(f.store.begin_answer_execution(&claim).await.unwrap());
        }
        sqlx::query("UPDATE answer_executions SET started_ms=started_ms-90000,deadline_ms=started_ms,sent_ms=CASE WHEN sent_ms IS NULL THEN NULL ELSE started_ms-1 END WHERE user_id=$1 AND request_id=$2")
            .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&input.request_id).unwrap())
            .execute(&f.pool).await.unwrap();
        let list = f.store.list_answer_executions(&f.owner).await.unwrap();
        assert_eq!(list[0].status, "unknown");
        assert!(list[0].preview.is_none() && list[0].result.is_none());
        assert!(!f.store.begin_answer_execution(&claim).await.unwrap());
        assert!(!f.store.check_answer_execution(&claim).await.unwrap());
        let finished = f
            .store
            .finish_answer_execution(&claim, AnswerExecutionOutcome::Succeeded(result()))
            .await
            .unwrap();
        assert_eq!(finished.status, "unknown");
        assert!(finished.result.is_none() && finished.preview.is_none());
        assert!(
            f.store
                .claim_answer_execution(&f.owner, &input.request_id, &claim.digest)
                .await
                .unwrap()
                .is_none()
        );
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_execution_rejects_forged_claims_and_success_without_send() {
    let f = Fixture::new().await;
    let input = f.input();
    let claim = f.claim(&input).await;
    let mut wrong_token = copy_claim(&claim);
    wrong_token.token = Uuid::new_v4().to_string();
    let mut wrong_digest = copy_claim(&claim);
    wrong_digest.digest = "0".repeat(64);
    let mut wrong_preview = copy_claim(&claim);
    wrong_preview.preview.question.push('!');
    let mut wrong_request = copy_claim(&claim);
    wrong_request.preview.request_sha256 = "0".repeat(64);
    for forged in [wrong_token, wrong_digest, wrong_preview, wrong_request] {
        assert!(matches!(
            f.store.begin_answer_execution(&forged).await,
            Err(StorageError::Conflict(_))
        ));
        assert!(matches!(
            f.store
                .finish_answer_execution(&forged, AnswerExecutionOutcome::Unknown)
                .await,
            Err(StorageError::Conflict(_))
        ));
    }
    assert!(matches!(
        f.store
            .finish_answer_execution(&claim, AnswerExecutionOutcome::Succeeded(result()))
            .await,
        Err(StorageError::Conflict(_))
    ));
    let running = f
        .store
        .get_answer_execution(&f.owner, &input.request_id)
        .await
        .unwrap();
    assert_eq!(running.status, "running");
    assert!(running.sent_at_unix_ms.is_none() && running.result.is_none());
    assert!(f.store.begin_answer_execution(&claim).await.unwrap());
    let mut invalid = result();
    invalid.citations[0].quote = "fabricated private secret".into();
    let failed = f
        .store
        .finish_answer_execution(&claim, AnswerExecutionOutcome::Succeeded(invalid))
        .await
        .unwrap();
    assert_eq!(failed.status, "failed");
    assert!(failed.result.is_none() && failed.preview.is_none());
    assert!(!serde_json::to_string(&failed).unwrap().contains("secret"));
    assert!(!f.store.begin_answer_execution(&claim).await.unwrap());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_execution_cancellation_discards_running_and_completed_results() {
    for finished in [false, true] {
        let f = Fixture::new().await;
        let input = f.input();
        let claim = f.claim(&input).await;
        assert!(f.store.begin_answer_execution(&claim).await.unwrap());
        if finished {
            f.store
                .finish_answer_execution(&claim, AnswerExecutionOutcome::Succeeded(result()))
                .await
                .unwrap();
        }
        let cancelled = f
            .store
            .cancel_answer_execution(&f.owner, &input.request_id)
            .await
            .unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert!(cancelled.result.is_none() && cancelled.preview.is_none());
        let late = f
            .store
            .finish_answer_execution(&claim, AnswerExecutionOutcome::Succeeded(result()))
            .await
            .unwrap();
        assert_eq!(late.status, "cancelled");
        assert!(late.result.is_none());
        assert!(!f.store.begin_answer_execution(&claim).await.unwrap());
        assert!(!f.store.check_answer_execution(&claim).await.unwrap());
        assert_eq!(
            f.store
                .get_answer_authorization(&f.owner, &input.request_id)
                .await
                .unwrap()
                .status,
            "consumed"
        );
        assert!(
            f.store
                .claim_answer_execution(&f.owner, &input.request_id, &claim.digest)
                .await
                .unwrap()
                .is_none()
        );
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_execution_failures_and_lost_responses_do_not_restore_authorization() {
    for outcome in [
        AnswerExecutionOutcome::Failed,
        AnswerExecutionOutcome::Unknown,
    ] {
        let expected = if matches!(outcome, AnswerExecutionOutcome::Failed) {
            "failed"
        } else {
            "unknown"
        };
        let f = Fixture::new().await;
        let input = f.input();
        let claim = f.claim(&input).await;
        assert!(f.store.begin_answer_execution(&claim).await.unwrap());
        let item = f
            .store
            .finish_answer_execution(&claim, outcome)
            .await
            .unwrap();
        assert_eq!(item.status, expected);
        assert!(item.result.is_none() && item.preview.is_none());
        let late = f
            .store
            .finish_answer_execution(&claim, AnswerExecutionOutcome::Succeeded(result()))
            .await
            .unwrap();
        assert_eq!(late.status, expected);
        assert!(!f.store.begin_answer_execution(&claim).await.unwrap());
        assert!(
            f.store
                .claim_answer_execution(&f.owner, &input.request_id, &claim.digest)
                .await
                .unwrap()
                .is_none()
        );
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_execution_audit_failures_roll_back_atomic_state_changes() {
    let f = Fixture::new().await;
    let input = f.input();
    let authorization = f.authorize(&input).await;
    sqlx::query("INSERT INTO answer_authorization_audit(user_id,request_id,event,at_ms) VALUES($1,$2,'consumed',1)")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&input.request_id).unwrap())
        .execute(&f.pool).await.unwrap();
    assert!(
        f.store
            .claim_answer_execution(&f.owner, &input.request_id, &authorization.digest)
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
    assert!(matches!(
        f.store
            .get_answer_execution(&f.owner, &input.request_id)
            .await,
        Err(StorageError::NotFound)
    ));
    let input = f.input();
    let claim = f.claim(&input).await;
    sqlx::query("INSERT INTO answer_execution_audit(user_id,request_id,event,at_ms) VALUES($1,$2,'sending',1)")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&input.request_id).unwrap())
        .execute(&f.pool).await.unwrap();
    assert!(f.store.begin_answer_execution(&claim).await.is_err());
    assert!(
        f.store
            .get_answer_execution(&f.owner, &input.request_id)
            .await
            .unwrap()
            .sent_at_unix_ms
            .is_none()
    );
    let input = f.input();
    let claim = f.claim(&input).await;
    assert!(f.store.begin_answer_execution(&claim).await.unwrap());
    sqlx::query("INSERT INTO answer_execution_audit(user_id,request_id,event,at_ms) VALUES($1,$2,'succeeded',1)")
        .bind(Uuid::parse_str(f.owner.as_str()).unwrap()).bind(Uuid::parse_str(&input.request_id).unwrap())
        .execute(&f.pool).await.unwrap();
    assert!(
        f.store
            .finish_answer_execution(&claim, AnswerExecutionOutcome::Succeeded(result()))
            .await
            .is_err()
    );
    let running = f
        .store
        .get_answer_execution(&f.owner, &input.request_id)
        .await
        .unwrap();
    assert_eq!(running.status, "running");
    assert!(running.sent_at_unix_ms.is_some() && running.result.is_none());
    assert!(!f.store.begin_answer_execution(&claim).await.unwrap());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn answer_execution_does_not_upgrade_a_legacy_consumed_authorization() {
    let f = Fixture::new().await;
    let input = f.input();
    let authorization = f.authorize(&input).await;
    assert!(
        f.store
            .claim_answer(&f.owner, &input.request_id, &authorization.digest)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        f.store
            .claim_answer_execution(&f.owner, &input.request_id, &authorization.digest)
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        f.store
            .get_answer_execution(&f.owner, &input.request_id)
            .await,
        Err(StorageError::NotFound)
    ));
    f.cleanup().await;
}
