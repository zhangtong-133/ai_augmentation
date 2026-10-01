use super::*;
use personal_ai_agent_core::model_executor::{
    ModelAgentExecutor, ModelAgentProvider, ModelChatStage, ModelEmbeddingCompletion,
    ModelRetriever,
};
use personal_ai_llm::{ChatRequest, Embedding};
use personal_ai_storage::{
    StorageResult,
    model_agents::{
        AgentBudgetLimits, ModelCallBudget, ModelPlanningConfiguration, ModelPlanningStore,
    },
    model_execution::{ModelExecutionConfiguration, ModelExecutionStore, RetrievedChunk},
};
struct Provider(AtomicUsize);
impl ModelAgentProvider for Provider {
    fn chat<'a>(
        &'a self,
        stage: ModelChatStage,
        _: &'a ChatRequest,
        _: &'a ModelCallBudget,
    ) -> BoxFuture<'a, Result<ReplyCompletion, ReplySendError>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ReplyCompletion {
                content: if stage == ModelChatStage::Planning {
                    r#"{"searches":[{"query":"research","limit":5}]}"#
                } else {
                    r#"{"insufficient_evidence":false,"answer":"grounded","citations":[1]}"#
                }
                .into(),
                usage: None,
            })
        })
    }
    fn embed<'a>(
        &'a self,
        _: &'a str,
        b: &'a ModelCallBudget,
    ) -> BoxFuture<'a, Result<ModelEmbeddingCompletion, ReplySendError>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ModelEmbeddingCompletion {
                embedding: Embedding {
                    model: b.model.clone(),
                    values: vec![1.0, 0.0],
                },
                usage: None,
            })
        })
    }
}
struct Retrieval(Vec<RetrievedChunk>);
impl ModelRetriever for Retrieval {
    fn retrieve<'a>(
        &'a self,
        _: &'a UserId,
        _: &'a Embedding,
        _: usize,
    ) -> BoxFuture<'a, StorageResult<Vec<RetrievedChunk>>> {
        Box::pin(async { Ok(self.0.clone()) })
    }
}
async fn setup() -> (Fixture, Arc<Provider>) {
    setup_with_evidence(false).await
}
async fn setup_with_evidence(with_evidence: bool) -> (Fixture, Arc<Provider>) {
    let mut f = Fixture::new().await;
    let version = Uuid::new_v4().to_string();
    let limits = AgentBudgetLimits {
        phase_amount: 10000,
        daily_amount: 100_000,
        daily_model_calls: 100,
        daily_tool_calls: 100,
    };
    let planning = ModelPlanningConfiguration {
        limits,
        budget: ModelCallBudget {
            configuration_version: format!("{version}-p"),
            provider: "fixture".into(),
            model: "fixture-model".into(),
            price_version: "v1".into(),
            counter_version: "v1".into(),
            currency: "USD".into(),
            input_price_per_million: 1_000_000,
            output_price_per_million: 1_000_000,
            input_token_bound: 100,
            output_token_bound: 2048,
            valid_until_unix_ms: 4_102_444_800_000,
        },
    };
    let mut answer = planning.budget.clone();
    answer.configuration_version = format!("{version}-a");
    answer.output_token_bound = 1024;
    let mut embedding = answer.clone();
    embedding.configuration_version = format!("{version}-e");
    embedding.output_token_bound = 0;
    embedding.output_price_per_million = 0;
    let execution = ModelExecutionConfiguration {
        version: answer.configuration_version.clone(),
        answer,
        embedding,
        limits,
    };
    f.store
        .register_model_planning_configuration(&planning)
        .await
        .unwrap();
    f.store
        .register_model_execution_configuration(&execution)
        .await
        .unwrap();
    let hits = if with_evidence {
        vec![insert_evidence(&f).await]
    } else {
        vec![]
    };
    let provider = Arc::new(Provider(AtomicUsize::new(0)));
    f.state.model_agents = Some(Arc::new(crate::ModelAgentRuntime {
        store: f.store.clone(),
        planning_version: planning.budget.configuration_version.clone(),
        execution_version: execution.version.clone(),
        executor: Some(Arc::new(ModelAgentExecutor::new(
            f.store.clone(),
            provider.clone(),
            Arc::new(Retrieval(hits)),
            planning,
            execution,
        ))),
    }));
    (f, provider)
}
fn consent(q: &serde_json::Value) -> serde_json::Value {
    json!({"digest":q["digest"],"accepted_currency":q["currency"],"accepted_amount_micro":q["amount_micro"],"accepted_calls":q["calls"],"acknowledge_cost":true})
}
async fn wait(f: &Fixture, path: &str, phase: &str, status: &str) -> serde_json::Value {
    for _ in 0..100 {
        let (_, item) = f
            .call("GET", path, json!(null), Some(&f.cookie), false)
            .await;
        if item[phase]["status"] == status {
            return item[phase].clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("model agent did not reach expected state");
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
#[allow(clippy::too_many_lines)] // 完整两阶段 HTTP 事务验收。
async fn model_http_two_stage_consent_is_exact_and_replays_do_not_dispatch() {
    let (mut f, provider) = setup().await;
    let base = format!("/api/conversations/{}/model-agents", f.conversation);
    let id = Uuid::new_v4().to_string();
    let path = format!("{base}/{id}");
    let input = json!({"request_id":id,"expected_revision":1});
    for (cookie, csrf, status) in [
        (None, true, StatusCode::UNAUTHORIZED),
        (Some(f.cookie.as_str()), false, StatusCode::FORBIDDEN),
    ] {
        assert_eq!(
            f.call("POST", &base, input.clone(), cookie, csrf).await.0,
            status
        );
    }
    let (status, q) = f
        .call("POST", &base, input.clone(), Some(&f.cookie), true)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(q["amount_micro"], "2148");
    assert!(q.get("amount").is_none());
    for extra in [
        json!({"model":"forged"}),
        json!({"searches":[]}),
        json!({"price":0}),
    ] {
        let mut body = input.clone();
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            f.call("POST", &base, body, Some(&f.cookie), true).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let authorize = format!("{path}/approve-planning");
    for (field, value) in [
        ("accepted_amount_micro", json!("2147")),
        ("digest", json!("wrong")),
        ("acknowledge_cost", json!(false)),
        ("accepted_currency", json!("EUR")),
    ] {
        let mut body = consent(&q);
        body[field] = value;
        assert_eq!(
            f.call("POST", &authorize, body, Some(&f.cookie), true)
                .await
                .0,
            StatusCode::CONFLICT
        );
    }
    assert_eq!(provider.0.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.call("POST", &authorize, consent(&q), Some(&f.cookie), true)
            .await
            .0,
        StatusCode::ACCEPTED
    );
    wait(&f, &path, "planning", "succeeded").await;
    f.call("POST", &authorize, consent(&q), Some(&f.cookie), true)
        .await;
    assert_eq!(provider.0.load(Ordering::SeqCst), 1);
    let (status, execution) = f
        .call(
            "POST",
            &format!("{path}/preview-execution"),
            json!({}),
            Some(&f.cookie),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(execution["amount_micro"], "1224");
    assert_eq!(provider.0.load(Ordering::SeqCst), 1);
    let authorize = format!("{path}/approve-execution");
    assert_eq!(
        f.call("POST", &authorize, consent(&q), Some(&f.cookie), true)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.call(
            "POST",
            &authorize,
            consent(&execution),
            Some(&f.cookie),
            true
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    wait(&f, &path, "execution", "insufficient_evidence").await;
    f.call(
        "POST",
        &authorize,
        consent(&execution),
        Some(&f.cookie),
        true,
    )
    .await;
    assert_eq!(provider.0.load(Ordering::SeqCst), 2);
    assert_eq!(f.occupied().await, 2248);
    let original = f.state.model_agents.as_ref().unwrap();
    f.state.model_agents = Some(Arc::new(crate::ModelAgentRuntime {
        store: original.store.clone(),
        executor: None,
        planning_version: String::new(),
        execution_version: String::new(),
    }));
    let (_, history) = f
        .call("GET", &base, json!(null), Some(&f.cookie), false)
        .await;
    assert_eq!(history["enabled"], false);
    assert_eq!(history["items"].as_array().unwrap().len(), 1);
    assert!(!history.to_string().contains("configuration_version"));
    assert!(!history.to_string().contains("claim_id"));
    assert_eq!(
        f.call(
            "POST",
            &base,
            json!({"request_id":Uuid::new_v4(),"expected_revision":1}),
            Some(&f.cookie),
            true
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_http_isolates_users_rejects_stale_revision_and_cancels_disabled_requests() {
    let (mut f, provider) = setup().await;
    let other = Fixture::new().await;
    let base = format!("/api/conversations/{}/model-agents", f.conversation);
    let id = Uuid::new_v4();
    let path = format!("{base}/{id}");
    let (_, q) = f
        .call(
            "POST",
            &base,
            json!({"request_id":id,"expected_revision":1}),
            Some(&f.cookie),
            true,
        )
        .await;
    for endpoint in [&base, &path] {
        assert_eq!(
            f.call("GET", endpoint, json!(null), Some(&other.cookie), false)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        f.call(
            "POST",
            &format!("{path}/approve-planning"),
            consent(&q),
            Some(&other.cookie),
            true
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    f.store
        .append_message(
            &f.user.id,
            &f.conversation,
            &Uuid::new_v4().to_string(),
            "changed",
        )
        .await
        .unwrap();
    assert_eq!(
        f.call(
            "POST",
            &format!("{path}/approve-planning"),
            consent(&q),
            Some(&f.cookie),
            true
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let original = f.state.model_agents.as_ref().unwrap();
    f.state.model_agents = Some(Arc::new(crate::ModelAgentRuntime {
        store: original.store.clone(),
        executor: None,
        planning_version: String::new(),
        execution_version: String::new(),
    }));
    assert_eq!(
        f.call(
            "POST",
            &format!("{path}/cancel-planning"),
            json!({}),
            Some(&f.cookie),
            false
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, result) = f
        .call(
            "POST",
            &format!("{path}/cancel-planning"),
            json!({}),
            Some(&f.cookie),
            true,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(result["status"], "cancelled");
    assert_eq!(provider.0.load(Ordering::SeqCst), 0);
    assert_eq!(f.occupied().await, 0);
    f.cleanup().await;
    other.cleanup().await;
}

async fn insert_evidence(f: &Fixture) -> RetrievedChunk {
    use personal_ai_storage::documents::{DocumentStore, DocumentSummary, StoredDocument};
    let id = Uuid::new_v4().to_string();
    f.store
        .insert_document(
            &f.user.id,
            &id,
            &StoredDocument {
                summary: DocumentSummary {
                    id: id.clone(),
                    title: "Private evidence".into(),
                    source: "local.md".into(),
                    source_type: "markdown".into(),
                    tags: vec![],
                    created_at_unix_ms: 1,
                    chunk_count: 1,
                },
                markdown: "verified evidence".into(),
                original_pdf: None,
                original_html: None,
                chunks: vec!["verified evidence".into()],
            },
        )
        .await
        .unwrap();
    RetrievedChunk {
        document_id: id,
        ordinal: 0,
        text: "verified evidence".into(),
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn model_http_persists_answer_citations_and_rejects_imprecise_amount_encoding() {
    let (f, provider) = setup_with_evidence(true).await;
    let base = format!("/api/conversations/{}/model-agents", f.conversation);
    let id = Uuid::new_v4();
    let path = format!("{base}/{id}");
    let (_, q) = f
        .call(
            "POST",
            &base,
            json!({"request_id":id,"expected_revision":1}),
            Some(&f.cookie),
            true,
        )
        .await;
    for amount in [
        json!(2148),
        json!("02148"),
        json!("2.148e3"),
        json!("9223372036854775808"),
    ] {
        let mut body = consent(&q);
        body["accepted_amount_micro"] = amount;
        assert_eq!(
            f.call(
                "POST",
                &format!("{path}/approve-planning"),
                body,
                Some(&f.cookie),
                true
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(provider.0.load(Ordering::SeqCst), 0);
    f.call(
        "POST",
        &format!("{path}/approve-planning"),
        consent(&q),
        Some(&f.cookie),
        true,
    )
    .await;
    wait(&f, &path, "planning", "succeeded").await;
    let (_, q) = f
        .call(
            "POST",
            &format!("{path}/preview-execution"),
            json!({}),
            Some(&f.cookie),
            true,
        )
        .await;
    f.call(
        "POST",
        &format!("{path}/approve-execution"),
        consent(&q),
        Some(&f.cookie),
        true,
    )
    .await;
    let result = wait(&f, &path, "execution", "succeeded").await;
    assert_eq!(result["answer"]["answer"], "grounded");
    assert_eq!(result["answer"]["citations"], json!([1]));
    assert_eq!(result["evidence"][0]["text"], "verified evidence");
    assert_eq!(result["evidence"][0]["title"], "Private evidence");
    assert_eq!(provider.0.load(Ordering::SeqCst), 3);
    assert_eq!(f.occupied().await, 3372);
    assert!(!result.to_string().contains("claim_id"));
    f.cleanup().await;
}
