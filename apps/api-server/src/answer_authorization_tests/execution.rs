use super::*;
use axum::{extract::State, http::header, routing::post};
use personal_ai_llm::{
    local::LocalTarget,
    local_answer::{self, AnswerBackend},
};
use personal_ai_storage::{
    answer_authorizations::AnswerAuthorizationStore, answer_executions::AnswerExecutionStore,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;

const BACKENDS: [AnswerBackend; 2] = [AnswerBackend::Local, AnswerBackend::Ollama];

struct Guard(AtomicBool);
impl crate::AnswerExecutionGuard for Guard {
    fn check(&self) -> personal_ai_llm::BoxFuture<'_, Result<(), &'static str>> {
        Box::pin(async move {
            if self.0.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err("fixture resource denied")
            }
        })
    }
}
struct ModelFixture {
    backend: AnswerBackend,
    mode: usize,
    calls: AtomicUsize,
    requests: Mutex<Vec<serde_json::Value>>,
    started: Notify,
    release: Notify,
}
async fn inference(
    State(model): State<Arc<ModelFixture>>,
    axum::Json(input): axum::Json<serde_json::Value>,
) -> impl axum::response::IntoResponse {
    model.calls.fetch_add(1, Ordering::SeqCst);
    model.requests.lock().unwrap().push(input);
    model.started.notify_one();
    if model.mode == 1 {
        model.release.notified().await;
    }
    let result = match model.mode {
        2 => {
            json!({"answer":"不得展示的错误结果","citations":[{"id":1,"quote":"不存在的引用"}],"insufficient_evidence":false})
        }
        3 => json!({"answer":"","citations":[],"insufficient_evidence":true}),
        _ => {
            json!({"answer":"会议安排在周五。","citations":[{"id":1,"quote":"会议安排在周五。"}],"insufficient_evidence":false})
        }
    };
    let (content_type, wire) = match model.backend {
        AnswerBackend::Local => {
            let record = json!({"model":"answer-fixture","choices":[{"index":0,"delta":{"content":result.to_string()},"finish_reason":"stop"}]});
            let wire = if model.mode == 4 {
                format!("data: {record}\n\n")
            } else {
                format!("data: {record}\n\ndata: [DONE]\n\n")
            };
            ("text/event-stream", wire)
        }
        AnswerBackend::Ollama => {
            let record = json!({"model":"answer-fixture","message":{"role":"assistant","content":result.to_string()},"done":false});
            let wire = if model.mode == 4 {
                format!("{record}\n")
            } else {
                let done = json!({"model":"answer-fixture","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop"});
                format!("{record}\n{done}\n")
            };
            ("application/x-ndjson", wire)
        }
    };
    ([(header::CONTENT_TYPE, content_type)], wire)
}
struct Fixture {
    state: AppState,
    auth: Arc<MemoryStore>,
    pg: Arc<PostgresStore>,
    pool: sqlx::PgPool,
    owner: UserId,
    cookie: String,
    document: StoredDocument,
    target: LocalTarget,
    backend: AnswerBackend,
    model: Arc<ModelFixture>,
    guard: Arc<Guard>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(mode: usize) -> Self {
        Self::new_for(mode, AnswerBackend::Local).await
    }
    async fn new_for(mode: usize, backend: AnswerBackend) -> Self {
        let (mut state, auth, _, owner, cookie, mut document) = retrieval_fixture().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = LocalTarget::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            "answer-fixture",
        )
        .unwrap();
        let model = Arc::new(ModelFixture {
            backend,
            mode,
            calls: AtomicUsize::new(0),
            requests: Mutex::new(vec![]),
            started: Notify::new(),
            release: Notify::new(),
        });
        let app = axum::Router::new()
            .route(
                match backend {
                    AnswerBackend::Local => "/v1/chat/completions",
                    AnswerBackend::Ollama => "/api/chat",
                },
                post(inference),
            )
            .with_state(model.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let pg = Arc::new(PostgresStore::connect(&url).await.unwrap());
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        pg.save_user(&User {
            id: owner.clone(),
            email: format!("{}@execution.example", owner.as_str()),
            display_name: "执行测试".into(),
        })
        .await
        .unwrap();
        document.chunks = vec!["开场🙂：会议安排在周五。尾声 e\u{301}。".into()];
        pg.insert_document(&owner, "execution-fixture", &document)
            .await
            .unwrap();
        let guard = Arc::new(Guard(AtomicBool::new(true)));
        state.answer_authorizations = Some(pg.clone());
        state.answer_executions = Some(pg.clone());
        let provider: Arc<dyn personal_ai_llm::AnswerProvider> = match backend {
            AnswerBackend::Local => {
                Arc::new(personal_ai_llm_local::answer::LocalAnswers::new(target.clone()).unwrap())
            }
            AnswerBackend::Ollama => Arc::new(
                personal_ai_llm_local::ollama_local_answer::OllamaLocalAnswers::new(target.clone())
                    .unwrap(),
            ),
        };
        state.answer_execution = Some(Arc::new(crate::AnswerExecutionRuntime::new_for(
            backend,
            target.clone(),
            provider,
            guard.clone(),
        )));
        state.indexing = None;
        Self {
            state,
            auth,
            pg,
            pool,
            owner,
            cookie,
            document,
            target,
            backend,
            model,
            guard,
            server,
        }
    }
    async fn request(
        &self,
        method: &str,
        suffix: &str,
        input: serde_json::Value,
        expected: StatusCode,
    ) -> serde_json::Value {
        let response = auth_request(
            router(self.state.clone()),
            method,
            &format!("{BASE}{suffix}"),
            Some(&self.cookie),
            &input.to_string(),
            true,
        )
        .await;
        let status = response.status();
        assert_eq!(response.headers()["cache-control"], "no-store");
        let result = json_body(response).await;
        assert_eq!(
            status, expected,
            "{method} {suffix}, {:?} mode {}: {result}",
            self.backend, self.model.mode
        );
        result
    }
    async fn authorized(&self, model: &str) -> serde_json::Value {
        self.authorized_for(model, self.backend).await
    }
    async fn authorized_for(&self, model: &str, backend: AnswerBackend) -> serde_json::Value {
        let id = Uuid::new_v4().to_string();
        let mut input = json!({"request_id":id,"question":"会议什么时候？","endpoint":self.target.endpoint(),"model":model,"sources":[{"document_id":self.document.summary.id,"ordinal":0}]});
        if !backend.is_local() {
            input["backend"] = json!(backend);
        }
        let prepared = self.request("POST", "", input, StatusCode::OK).await;
        assert_eq!(self.model.calls.load(Ordering::SeqCst), 0);
        let authorized = self.request("POST",&format!("/{id}/approve"),json!({"digest":prepared["digest"],"acknowledge_sharing":true,"acknowledge_local_compute":true}),StatusCode::OK).await;
        assert_eq!(authorized["status"], "authorized");
        assert_eq!(self.model.calls.load(Ordering::SeqCst), 0);
        authorized
    }
    async fn execute(&self, grant: &serde_json::Value, expected: StatusCode) -> serde_json::Value {
        self.request(
            "POST",
            &format!("/{}/execute", grant["request_id"].as_str().unwrap()),
            json!({"digest":grant["digest"]}),
            expected,
        )
        .await
    }
    async fn terminal(&self, id: &str) -> serde_json::Value {
        // A detail operation itself can wait up to 15 seconds. Do not cancel it
        // after only five seconds when the CI runner is busy scheduling tests.
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                let item = self
                    .request("GET", &format!("/{id}"), json!({}), StatusCode::OK)
                    .await;
                if item["execution"]["status"] != "running" {
                    return item;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "execution did not finish: {:?} mode {}",
                self.backend, self.model.mode
            )
        })
    }
    async fn cleanup(self) {
        self.model.release.notify_waiters();
        self.server.abort();
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(Uuid::parse_str(self.owner.as_str()).unwrap())
            .execute(&self.pool)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 与 loopback HTTP"]
async fn local_answer_execution_sends_exact_preview_once_and_recovers_persisted_citations() {
    for backend in BACKENDS {
        let f = Fixture::new_for(0, backend).await;
        let grant = f.authorized("answer-fixture").await;
        let id = grant["request_id"].as_str().unwrap();
        let result = f.execute(&grant, StatusCode::ACCEPTED).await;
        assert_eq!(result["status"], "consumed");
        let terminal = f.terminal(id).await;
        assert_eq!(terminal["execution"]["status"], "succeeded");
        assert_eq!(terminal["execution"]["result"]["status"], "answered");
        let citation = &terminal["execution"]["result"]["citations"][0];
        let start = usize::try_from(citation["quote_start"].as_u64().unwrap()).unwrap();
        let end = usize::try_from(citation["quote_end"].as_u64().unwrap()).unwrap();
        assert_eq!(
            f.document.chunks[0]
                .chars()
                .skip(start)
                .take(end - start)
                .collect::<String>(),
            citation["quote"].as_str().unwrap()
        );
        assert_eq!(
            f.model.requests.lock().unwrap()[0],
            grant["preview"]["request"]["body"]
        );
        let sources = [personal_ai_llm::AnswerSource {
            id: 1,
            text: f.document.chunks[0].clone(),
        }];
        assert_eq!(
            local_answer::preview_for(backend, &f.target, "会议什么时候？", &sources)
                .unwrap()
                .fingerprint()
                .unwrap(),
            grant["preview"]["request_sha256"]
        );
        f.execute(&grant, StatusCode::OK).await;
        assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
        let listed = f.request("GET", "", json!({}), StatusCode::OK).await;
        assert!(listed["items"][0]["preview"].is_null());
        assert!(listed["items"][0]["execution"]["question"].is_null());
        assert!(listed["items"][0]["execution"]["result"].is_null());
        let reopened = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
        assert!(
            reopened
                .get_answer_execution(&f.owner, id)
                .await
                .unwrap()
                .result
                .is_some()
        );
        let deleted = f
            .request("POST", &format!("/{id}/cancel"), json!({}), StatusCode::OK)
            .await;
        assert_eq!(deleted["execution"]["status"], "cancelled");
        assert!(deleted["execution"]["result"].is_null());
        f.execute(&grant, StatusCode::OK).await;
        assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 与 loopback HTTP"]
async fn local_answer_execution_rejects_unapproved_target_resource_and_source_changes_without_sending()
 {
    for backend in BACKENDS {
        let f = Fixture::new_for(0, backend).await;
        let draft_id = Uuid::new_v4().to_string();
        let draft = f.request("POST","",json!({"request_id":draft_id,"backend":backend,"question":"会议什么时候？","endpoint":f.target.endpoint(),"model":"answer-fixture","sources":[{"document_id":f.document.summary.id,"ordinal":0}]}),StatusCode::OK).await;
        f.execute(&draft, StatusCode::CONFLICT).await;
        let wrong = f.authorized("different-model").await;
        f.execute(&wrong, StatusCode::CONFLICT).await;
        let grant = f.authorized("answer-fixture").await;
        let id = grant["request_id"].as_str().unwrap();
        f.request(
            "POST",
            &format!("/{id}/execute"),
            json!({"digest":"wrong"}),
            StatusCode::CONFLICT,
        )
        .await;
        f.guard.0.store(false, Ordering::SeqCst);
        f.execute(&grant, StatusCode::SERVICE_UNAVAILABLE).await;
        assert_eq!(
            f.pg.get_answer_authorization(&f.owner, id)
                .await
                .unwrap()
                .status,
            "authorized"
        );
        f.guard.0.store(true, Ordering::SeqCst);
        sqlx::query("UPDATE documents SET title='changed' WHERE id=$1")
            .bind(Uuid::parse_str(&f.document.summary.id).unwrap())
            .execute(&f.pool)
            .await
            .unwrap();
        f.execute(&grant, StatusCode::CONFLICT).await;
        assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 与 loopback HTTP"]
async fn local_answer_execution_invalid_outputs_fail_and_insufficient_results_remain_distinct() {
    for backend in BACKENDS {
        for (mode, expected) in [(2, "failed"), (3, "succeeded"), (4, "failed")] {
            let f = Fixture::new_for(mode, backend).await;
            let grant = f.authorized("answer-fixture").await;
            f.execute(&grant, StatusCode::ACCEPTED).await;
            let terminal = f.terminal(grant["request_id"].as_str().unwrap()).await;
            assert_eq!(terminal["execution"]["status"], expected);
            if mode == 3 {
                assert_eq!(
                    terminal["execution"]["result"]["status"],
                    "insufficient_evidence"
                );
            } else {
                assert!(terminal["execution"]["result"].is_null());
                assert!(!terminal.to_string().contains("不得展示"));
            }
            f.execute(&grant, StatusCode::OK).await;
            assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
            f.cleanup().await;
        }
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 与 loopback HTTP"]
async fn local_answer_execution_cancellation_source_revocation_and_logout_discard_inflight_results()
{
    for backend in BACKENDS {
        for interruption in 0..3 {
            let f = Fixture::new_for(1, backend).await;
            let grant = f.authorized("answer-fixture").await;
            let id = grant["request_id"].as_str().unwrap();
            f.execute(&grant, StatusCode::ACCEPTED).await;
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                f.model.started.notified(),
            )
            .await
            .unwrap();
            // Even a duplicate while the first request is running cannot issue another send.
            f.execute(&grant, StatusCode::OK).await;
            match interruption {
                0 => {
                    f.request("POST", &format!("/{id}/cancel"), json!({}), StatusCode::OK)
                        .await;
                }
                1 => {
                    sqlx::query("UPDATE documents SET chunks=ARRAY['changed'] WHERE id=$1")
                        .bind(Uuid::parse_str(&f.document.summary.id).unwrap())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                _ => f.auth.sessions.lock().unwrap().clear(),
            }
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    let execution = f.pg.get_answer_execution(&f.owner, id).await.unwrap();
                    if execution.status != "running" {
                        assert_eq!(
                            execution.status,
                            match interruption {
                                0 => "cancelled",
                                1 => "invalidated",
                                _ => "unknown",
                            }
                        );
                        assert!(execution.result.is_none());
                        assert!(execution.preview.is_none());
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            f.model.release.notify_waiters();
            assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
            f.cleanup().await;
        }
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 与 loopback HTTP"]
async fn local_authorization_cannot_be_consumed_or_sent_by_a_native_ollama_runtime() {
    let f = Fixture::new_for(0, AnswerBackend::Ollama).await;
    let grant = f
        .authorized_for("answer-fixture", AnswerBackend::Local)
        .await;
    let id = grant["request_id"].as_str().unwrap();
    assert_eq!(
        grant["preview"]["request"]["profile"],
        local_answer::PROFILE
    );
    assert_eq!(
        f.state.answer_execution.as_ref().unwrap().backend(),
        AnswerBackend::Ollama
    );
    f.execute(&grant, StatusCode::CONFLICT).await;
    f.execute(&grant, StatusCode::CONFLICT).await;
    assert_eq!(
        f.pg.get_answer_authorization(&f.owner, id)
            .await
            .unwrap()
            .status,
        "authorized"
    );
    assert!(matches!(
        f.pg.get_answer_execution(&f.owner, id).await,
        Err(personal_ai_storage::StorageError::NotFound)
    ));
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
    f.cleanup().await;
}

// Deterministically commits an execution in the window before the old
// authorization cancellation acquires its transaction lock.
struct ClaimBeforeCancellation {
    pg: Arc<PostgresStore>,
    claims: AtomicUsize,
}
impl AnswerAuthorizationStore for ClaimBeforeCancellation {
    fn prepare_answer(
        &self,
        owner: &UserId,
        input: &personal_ai_storage::answer_authorizations::AnswerPreparation,
    ) -> personal_ai_storage::BoxFuture<
        '_,
        personal_ai_storage::StorageResult<
            personal_ai_storage::answer_authorizations::AnswerAuthorization,
        >,
    > {
        self.pg.prepare_answer(owner, input)
    }
    fn get_answer_authorization(
        &self,
        owner: &UserId,
        request: &str,
    ) -> personal_ai_storage::BoxFuture<
        '_,
        personal_ai_storage::StorageResult<
            personal_ai_storage::answer_authorizations::AnswerAuthorization,
        >,
    > {
        self.pg.get_answer_authorization(owner, request)
    }
    fn list_answer_authorizations(
        &self,
        owner: &UserId,
    ) -> personal_ai_storage::BoxFuture<
        '_,
        personal_ai_storage::StorageResult<
            Vec<personal_ai_storage::answer_authorizations::AnswerAuthorization>,
        >,
    > {
        self.pg.list_answer_authorizations(owner)
    }
    fn approve_answer(
        &self,
        owner: &UserId,
        request: &str,
        approval: &personal_ai_storage::answer_authorizations::AnswerApproval,
    ) -> personal_ai_storage::BoxFuture<
        '_,
        personal_ai_storage::StorageResult<
            personal_ai_storage::answer_authorizations::AnswerAuthorization,
        >,
    > {
        self.pg.approve_answer(owner, request, approval)
    }
    fn cancel_answer(
        &self,
        owner: &UserId,
        request: &str,
    ) -> personal_ai_storage::BoxFuture<
        '_,
        personal_ai_storage::StorageResult<
            personal_ai_storage::answer_authorizations::AnswerAuthorization,
        >,
    > {
        let owner = owner.clone();
        let request = request.to_owned();
        Box::pin(async move {
            let original = self.pg.get_answer_authorization(&owner, &request).await?;
            assert_eq!(original.status, "authorized");
            assert!(
                self.pg
                    .claim_answer_execution(&owner, &request, &original.digest)
                    .await?
                    .is_some()
            );
            self.claims.fetch_add(1, Ordering::SeqCst);
            self.pg.cancel_answer(&owner, &request).await
        })
    }
    fn claim_answer(
        &self,
        owner: &UserId,
        request: &str,
        digest: &str,
    ) -> personal_ai_storage::BoxFuture<
        '_,
        personal_ai_storage::StorageResult<
            Option<personal_ai_storage::answer_authorizations::AnswerClaim>,
        >,
    > {
        self.pg.claim_answer(owner, request, digest)
    }
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL 与 loopback HTTP"]
async fn local_answer_cancellation_also_cancels_an_execution_claimed_in_the_authorization_window() {
    let mut f = Fixture::new(0).await;
    let grant = f.authorized("answer-fixture").await;
    let id = grant["request_id"].as_str().unwrap();
    let store = Arc::new(ClaimBeforeCancellation {
        pg: f.pg.clone(),
        claims: AtomicUsize::new(0),
    });
    f.state.answer_authorizations = Some(store.clone());
    let cancelled = f
        .request("POST", &format!("/{id}/cancel"), json!({}), StatusCode::OK)
        .await;
    assert_eq!(store.claims.load(Ordering::SeqCst), 1);
    assert_eq!(cancelled["status"], "consumed");
    assert_eq!(cancelled["execution"]["status"], "cancelled");
    assert!(cancelled["preview"].is_null());
    assert!(cancelled["execution"]["question"].is_null());
    assert!(cancelled["execution"]["result"].is_null());
    assert!(
        f.pg.claim_answer_execution(&f.owner, id, grant["digest"].as_str().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    f.execute(&grant, StatusCode::OK).await;
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
    f.cleanup().await;
}
