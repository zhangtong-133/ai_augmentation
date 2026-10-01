use super::*;
use personal_ai_agent_core::{
    BoxFuture,
    model_executor::{
        ModelAgentExecutor, ModelAgentProvider, ModelChatStage, ModelEmbeddingCompletion,
        ModelRetriever,
    },
    reply_executor::{ReplyCompletion, ReplySendError},
};
use personal_ai_llm::{ChatRequest, Embedding, Role};
use personal_ai_storage::StorageResult;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

struct Provider {
    calls: Mutex<Vec<ModelChatStage>>,
    embeddings: AtomicUsize,
    mode: u8,
    entered: Notify,
    released: Notify,
}
impl Provider {
    fn new(mode: u8) -> Self {
        Self {
            calls: Mutex::new(vec![]),
            embeddings: AtomicUsize::new(0),
            mode,
            entered: Notify::new(),
            released: Notify::new(),
        }
    }
}
impl ModelAgentProvider for Provider {
    fn chat<'a>(
        &'a self,
        stage: ModelChatStage,
        request: &'a ChatRequest,
        _: &'a ModelCallBudget,
    ) -> BoxFuture<'a, Result<ReplyCompletion, ReplySendError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(stage);
            assert_eq!(request.messages[0].role, Role::System);
            assert!(request.messages[1..].iter().all(|m| m.role == Role::User));
            match self.mode {
                1 => return Err(ReplySendError::Unknown),
                2 => return Err(ReplySendError::ContractViolation),
                8 => {
                    std::future::pending::<()>().await;
                }
                _ => (),
            }
            let content = if stage == ModelChatStage::Planning {
                if self.mode == 3 {
                    "not JSON"
                } else {
                    r#"{"searches":[{"query":"research","limit":5}]}"#
                }
            } else {
                let input: serde_json::Value =
                    serde_json::from_str(&request.messages.last().unwrap().content).unwrap();
                assert_eq!(input["evidence"][0]["id"], 1);
                r#"{"insufficient_evidence":false,"answer":"supported","citations":[1]}"#
            };
            Ok(ReplyCompletion {
                content: content.into(),
                usage: if self.mode == 9 {
                    None
                } else {
                    Some(ReplyUsage {
                        input_tokens: if self.mode == 4 { 101 } else { 10 },
                        output_tokens: 5,
                    })
                },
            })
        })
    }
    fn embed<'a>(
        &'a self,
        _: &'a str,
        budget: &'a ModelCallBudget,
    ) -> BoxFuture<'a, Result<ModelEmbeddingCompletion, ReplySendError>> {
        Box::pin(async move {
            self.embeddings.fetch_add(1, Ordering::SeqCst);
            if self.mode == 5 {
                self.entered.notify_one();
                self.released.notified().await;
            }
            if self.mode == 6 {
                return Err(ReplySendError::ContractViolation);
            }
            Ok(ModelEmbeddingCompletion {
                embedding: Embedding {
                    values: vec![0.5, 0.5],
                    model: budget.model.clone(),
                },
                usage: Some(ReplyUsage {
                    input_tokens: 10,
                    output_tokens: 0,
                }),
            })
        })
    }
}
struct Retriever {
    hits: Vec<RetrievedChunk>,
    calls: AtomicUsize,
    fail: bool,
}
impl ModelRetriever for Retriever {
    fn retrieve<'a>(
        &'a self,
        _: &'a UserId,
        _: &'a Embedding,
        _: usize,
    ) -> BoxFuture<'a, StorageResult<Vec<RetrievedChunk>>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(StorageError::Unavailable("fixture".into()));
            }
            Ok(self.hits.clone())
        })
    }
}
async fn setup(
    f: &Fixture,
    mode: u8,
    hits: Vec<RetrievedChunk>,
) -> (
    Arc<ModelAgentExecutor>,
    Arc<Provider>,
    Arc<Retriever>,
    ModelExecutionConfiguration,
) {
    let c = config(f);
    f.store
        .register_model_execution_configuration(&c)
        .await
        .unwrap();
    let provider = Arc::new(Provider::new(mode));
    let retriever = Arc::new(Retriever {
        hits,
        calls: AtomicUsize::new(0),
        fail: mode == 7,
    });
    (
        Arc::new(ModelAgentExecutor::new(
            f.store.clone(),
            provider.clone(),
            retriever.clone(),
            f.configuration.clone(),
            c.clone(),
        )),
        provider,
        retriever,
        c,
    )
}
async fn stage_two(
    f: &Fixture,
    request: &ModelPlanningRequest,
    c: &ModelExecutionConfiguration,
) -> ModelExecutionRequest {
    f.store
        .create_model_execution_request(&f.owner, &f.conversation, &request.request_id, &c.version)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn executor_requires_two_approvals_and_concurrent_replays_never_resend() {
    let f = Fixture::new(20000, 100).await;
    let (executor, provider, retriever, c) = setup(&f, 0, vec![document(&f, &f.owner).await]).await;
    let request = f.draft().await;
    let consent_one = approval(&request);
    let (a, b) = tokio::join!(
        executor.approve_planning(&f.owner, &f.conversation, &request.request_id, &consent_one),
        executor.approve_planning(&f.owner, &f.conversation, &request.request_id, &consent_one)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(
        *provider.calls.lock().unwrap(),
        vec![ModelChatStage::Planning]
    );
    assert_eq!(provider.embeddings.load(Ordering::SeqCst), 0);
    let planning = f.get(&request).await;
    assert_eq!(planning.status, "succeeded");
    let second = stage_two(&f, &planning, &c).await;
    let consent_two = consent(&second);
    let (a, b) = tokio::join!(
        executor.approve_execution(&f.owner, &f.conversation, &second.request_id, &consent_two),
        executor.approve_execution(&f.owner, &f.conversation, &second.request_id, &consent_two)
    );
    a.unwrap();
    b.unwrap();
    let saved = executor
        .approve_execution(&f.owner, &f.conversation, &second.request_id, &consent_two)
        .await
        .unwrap();
    assert_eq!(saved.status, "succeeded");
    assert_eq!(saved.answer.unwrap().citations, vec![1]);
    assert_eq!(
        *provider.calls.lock().unwrap(),
        vec![ModelChatStage::Planning, ModelChatStage::Answer]
    );
    assert_eq!(provider.embeddings.load(Ordering::SeqCst), 1);
    assert_eq!(retriever.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        (f.money().await, f.calls().await, tool_count(&f).await),
        (40, 3, 1)
    );
    assert_eq!(f.audit().await.difference_micro, "0");
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn executor_planning_failure_and_contract_violation_stop_without_retries() {
    for mode in [1, 2, 3, 4, 9] {
        let f = Fixture::new(20000, 100).await;
        let (executor, provider, _, c) = setup(&f, mode, vec![]).await;
        let request = f.draft().await;
        let result = executor
            .approve_planning(
                &f.owner,
                &f.conversation,
                &request.request_id,
                &approval(&request),
            )
            .await
            .unwrap();
        assert_eq!(
            result.status,
            match mode {
                3 => "failed",
                9 => "succeeded",
                _ => "unknown",
            }
        );
        assert_eq!(f.money().await, AMOUNT);
        assert_eq!(executor.is_halted(), mode == 2 || mode == 4);
        if executor.is_halted() {
            assert!(
                f.store
                    .check_model_planning_configuration(&f.configuration)
                    .await
                    .is_err()
            );
            assert!(
                f.store
                    .check_model_execution_configuration(&c)
                    .await
                    .is_err()
            );
        }
        let _ = executor
            .approve_planning(
                &f.owner,
                &f.conversation,
                &request.request_id,
                &approval(&request),
            )
            .await;
        assert_eq!(provider.calls.lock().unwrap().len(), 1);
        assert_eq!(provider.embeddings.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn executor_empty_evidence_or_retrieval_failure_never_sends_answer() {
    for mode in [0, 6, 7] {
        let f = Fixture::new(20000, 100).await;
        let (executor, provider, retriever, c) = setup(&f, mode, vec![]).await;
        let planning = planned(&f, 1).await;
        let request = stage_two(&f, &planning, &c).await;
        let result = executor
            .approve_execution(
                &f.owner,
                &f.conversation,
                &request.request_id,
                &consent(&request),
            )
            .await
            .unwrap();
        assert_eq!(
            result.status,
            match mode {
                0 => "insufficient_evidence",
                6 => "unknown",
                _ => "failed",
            }
        );
        assert_eq!(f.money().await, AMOUNT + if mode == 0 { 10 } else { 100 });
        assert!(provider.calls.lock().unwrap().is_empty());
        assert_eq!(provider.embeddings.load(Ordering::SeqCst), 1);
        assert_eq!(
            retriever.calls.load(Ordering::SeqCst),
            usize::from(mode != 6)
        );
        assert_eq!(f.audit().await.difference_micro, "0");
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn executor_cancellation_during_embedding_stops_retrieval_and_answer() {
    let f = Fixture::new(20000, 100).await;
    let (executor, provider, retriever, c) = setup(&f, 5, vec![]).await;
    let planning = planned(&f, 1).await;
    let request = stage_two(&f, &planning, &c).await;
    let consent = consent(&request);
    let result = tokio::join!(
        executor.approve_execution(&f.owner, &f.conversation, &request.request_id, &consent),
        async {
            provider.entered.notified().await;
            cancel(&f, &request).await;
            provider.released.notify_one();
        }
    )
    .0
    .unwrap();
    assert_eq!(result.status, "cancelled");
    assert_eq!(retriever.calls.load(Ordering::SeqCst), 0);
    assert!(provider.calls.lock().unwrap().is_empty());
    assert_eq!(f.money().await, AMOUNT + 100);
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn executor_timeout_is_unknown_and_does_not_reissue() {
    let f = Fixture::new(20000, 100).await;
    let (executor, provider, _, _) = setup(&f, 8, vec![]).await;
    let request = f.draft().await;
    let result = executor
        .approve_planning(
            &f.owner,
            &f.conversation,
            &request.request_id,
            &approval(&request),
        )
        .await
        .unwrap();
    assert_eq!(result.status, "unknown");
    executor
        .approve_planning(
            &f.owner,
            &f.conversation,
            &request.request_id,
            &approval(&request),
        )
        .await
        .unwrap();
    assert_eq!(provider.calls.lock().unwrap().len(), 1);
    assert_eq!(f.money().await, AMOUNT);
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn executor_commit_failure_never_resends_after_replay_or_restart() {
    for status in ["dispatching", "succeeded"] {
        let f = Fixture::new(20000, 100).await;
        let (executor, provider, retriever, c) = setup(&f, 0, vec![]).await;
        let request = f.draft().await;
        let function = format!("executor_fail_{}", Uuid::new_v4().simple());
        let trigger = format!("executor_trigger_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE FUNCTION {function}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.request_id='{}'::uuid AND NEW.status='{status}' THEN RAISE EXCEPTION 'fixture commit failure'; END IF; RETURN NEW; END $$",request.request_id)).execute(&f.pool).await.unwrap();
        sqlx::query(&format!("CREATE CONSTRAINT TRIGGER {trigger} AFTER UPDATE ON model_planning_requests DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {function}()")).execute(&f.pool).await.unwrap();
        assert!(
            executor
                .approve_planning(
                    &f.owner,
                    &f.conversation,
                    &request.request_id,
                    &approval(&request)
                )
                .await
                .is_err()
        );
        sqlx::query(&format!(
            "DROP TRIGGER {trigger} ON model_planning_requests"
        ))
        .execute(&f.pool)
        .await
        .unwrap();
        sqlx::query(&format!("DROP FUNCTION {function}()"))
            .execute(&f.pool)
            .await
            .unwrap();
        let restarted = ModelAgentExecutor::new(
            f.store.clone(),
            provider.clone(),
            retriever,
            f.configuration.clone(),
            c,
        );
        restarted
            .approve_planning(
                &f.owner,
                &f.conversation,
                &request.request_id,
                &approval(&request),
            )
            .await
            .unwrap();
        assert_eq!(
            provider.calls.lock().unwrap().len(),
            usize::from(status == "succeeded")
        );
        assert_eq!(f.money().await, AMOUNT);
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn executor_configuration_switch_rejects_old_quotes_before_reserving_money() {
    let f = Fixture::new(20000, 100).await;
    let (_, provider, retriever, c) = setup(&f, 0, vec![]).await;
    let request = f.draft().await;
    let mut changed = f.configuration.clone();
    changed.budget.configuration_version = id();
    f.store
        .register_model_planning_configuration(&changed)
        .await
        .unwrap();
    let executor = Arc::new(ModelAgentExecutor::new(
        f.store.clone(),
        provider.clone(),
        retriever.clone(),
        changed,
        c.clone(),
    ));
    assert!(
        executor
            .start_planning(
                &f.owner,
                &f.conversation,
                &request.request_id,
                &approval(&request)
            )
            .await
            .is_err()
    );
    assert_eq!(f.money().await, 0);
    assert_eq!(f.get(&request).await.status, "draft");
    let planning = planned(&f, 1).await;
    let second = stage_two(&f, &planning, &c).await;
    let mut changed = c.clone();
    changed.version = id();
    changed.answer.configuration_version = changed.version.clone();
    f.store
        .register_model_execution_configuration(&changed)
        .await
        .unwrap();
    let executor = Arc::new(ModelAgentExecutor::new(
        f.store.clone(),
        provider.clone(),
        retriever,
        f.configuration.clone(),
        changed,
    ));
    assert!(
        executor
            .start_execution(
                &f.owner,
                &f.conversation,
                &second.request_id,
                &consent(&second)
            )
            .await
            .is_err()
    );
    assert_eq!(f.money().await, AMOUNT);
    assert_eq!(tool_count(&f).await, 0);
    assert!(provider.calls.lock().unwrap().is_empty());
    assert_eq!(provider.embeddings.load(Ordering::SeqCst), 0);
}
