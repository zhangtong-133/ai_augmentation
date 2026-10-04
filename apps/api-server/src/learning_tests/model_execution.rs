use super::reviews::{evidence, setup};
use super::*;
use personal_ai_agent_core::learning_model_execution::{
    ReviewRuntimeError, SubscriptionReviewRuntime, execute_model_review,
};
use personal_ai_llm::ChatRequest;
use personal_ai_storage::{
    BoxFuture,
    learning::model_authorization::{
        ModelAuthorization, ModelReviewClaim, ModelReviewExecutionStore,
    },
    subscription_connections::{SubscriptionConnectionStore, VerifiedSubscriptionConnection},
};
use std::sync::atomic::{AtomicUsize, Ordering};
async fn authorized() -> (Fixture, String, String, VerifiedSubscriptionConnection) {
    let (f, _, path, _) = setup().await;
    evidence(&f, &path).await;
    let time: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let proof = VerifiedSubscriptionConnection {
        host_id: Uuid::new_v4().urn().to_string(),
        client_id: format!("oaiapp_{}", Uuid::new_v4().simple()),
        subject: "learning-fixture".into(),
        label: "fixture".into(),
        models: vec!["fixture-model".into()],
        valid_until_unix_ms: time + 3_600_000,
    };
    let key = Uuid::new_v4();
    f.store
        .save_subscription_connection(&f.owner, &key.to_string(), 0, &proof)
        .await
        .unwrap();
    let request = Uuid::new_v4().to_string();
    let(status,item)=f.call("POST",&format!("{path}/model-authorizations"),json!({"request_id":request,"connection_id":key,"connection_revision":"1","model":"fixture-model"})).await;
    assert_eq!(status, StatusCode::OK);
    let url = format!("/api/learning/model-authorizations/{request}");
    assert_eq!(f.call("POST",&format!("{url}/approve"),json!({"digest":item["digest"],"acknowledge_sharing":true,"acknowledge_subscription_usage":true})).await.0,StatusCode::OK);
    (f, path, request, proof)
}
fn output(item: &ModelAuthorization) -> Vec<u8> {
    let p = item.preview.as_ref().unwrap();
    let fields = &p.input().evidence;
    let mut out = json!({"protocol_version":"learning-model-review-v1","input_digest":p.input().input_digest});
    for (field, quote) in [
        ("explanation", &fields.explanation),
        ("work", &fields.work),
        ("verification", &fields.verification),
        ("limitations", &fields.limitations),
    ] {
        out[field] = json!({"verdict":"supported","reason":"建议仍需人工核验 <script>","citations":[{"field":field,"quote":quote}]});
    }
    serde_json::to_vec(&out).unwrap()
}
struct Runtime {
    proof: VerifiedSubscriptionConnection,
    calls: AtomicUsize,
    fail: bool,
}
impl SubscriptionReviewRuntime for Runtime {
    fn verify<'a>(
        &'a self,
        _: &'a ModelAuthorization,
    ) -> BoxFuture<'a, Result<VerifiedSubscriptionConnection, ReviewRuntimeError>> {
        Box::pin(async move { Ok(self.proof.clone()) })
    }
    fn review<'a>(
        &'a self,
        item: &'a ModelAuthorization,
        request: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<Vec<u8>, ReviewRuntimeError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.messages.len(), 2);
            assert!(request.temperature.is_none());
            assert!(request.max_output_tokens.is_none());
            let shared: serde_json::Value =
                serde_json::from_str(&request.messages[1].content).unwrap();
            assert_eq!(
                shared["input_digest"],
                item.preview.as_ref().unwrap().input().input_digest
            );
            assert!(shared.get("connection_id").is_none());
            if self.fail {
                Err(ReviewRuntimeError)
            } else {
                Ok(output(item))
            }
        })
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_execution_is_once_only_private_and_does_not_change_self_assessments() {
    let (f, _, request, proof) = authorized().await;
    let before = f.call("GET", "/api/learning/snapshot", json!({})).await.1;
    let runtime = Runtime {
        proof,
        calls: AtomicUsize::new(0),
        fail: false,
    };
    let (a, b) = tokio::join!(
        execute_model_review(f.store.as_ref(), &runtime, &f.owner, &request),
        execute_model_review(f.store.as_ref(), &runtime, &f.owner, &request)
    );
    assert_eq!(
        usize::from(a.unwrap().is_some()) + usize::from(b.unwrap().is_some()),
        1
    );
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
    let item = f
        .store
        .get_model_authorization(&f.owner, &request)
        .await
        .unwrap();
    assert_eq!(item.status, "succeeded");
    assert!(item.advice.is_some());
    assert_eq!(
        f.call("GET", "/api/learning/snapshot", json!({})).await.1,
        before
    );
    let url = format!("/api/learning/model-authorizations/{request}");
    let (_, public) = f.call("GET", &url, json!({})).await;
    assert!(public["advice"]["work"]["citations"].is_array());
    assert!(public.get("dispatch_token").is_none());
    assert_eq!(
        f.send("GET", &url, json!({}), Some(&f.other_cookie), false)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let db = PostgresStore::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    assert!(
        db.claim_model_review(&f.owner, &request)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.call("POST", &format!("{url}/cancel"), json!({})).await.1["status"],
        "cancelled"
    );
    assert!(
        f.store
            .get_model_authorization(&f.owner, &request)
            .await
            .unwrap()
            .advice
            .is_none()
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_execution_rechecks_identity_and_persists_single_send_before_inference() {
    let (f, _, request, proof) = authorized().await;
    let claim = f
        .store
        .claim_model_review(&f.owner, &request)
        .await
        .unwrap()
        .unwrap();
    let mut wrong = proof.clone();
    wrong.subject = "another-account".into();
    assert!(f.store.begin_model_review(&claim, &wrong).await.is_err());
    let forged = ModelReviewClaim {
        owner: claim.owner.clone(),
        request_id: claim.request_id.clone(),
        token: Uuid::new_v4().to_string(),
        authorization: claim.authorization.clone(),
    };
    assert!(f.store.begin_model_review(&forged, &proof).await.is_err());
    let (a, b) = tokio::join!(
        f.store.begin_model_review(&claim, &proof),
        f.store.begin_model_review(&claim, &proof)
    );
    assert_eq!(usize::from(a.unwrap()) + usize::from(b.unwrap()), 1);
    let result = f
        .store
        .finish_model_review(&claim, Some(output(&claim.authorization)))
        .await
        .unwrap();
    assert_eq!(result.status, "succeeded");
    assert_eq!(
        f.store
            .finish_model_review(&claim, None)
            .await
            .unwrap()
            .status,
        "succeeded"
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_execution_discards_late_results_after_cancellation_or_source_revocation() {
    for mode in ["cancel", "evidence", "connection"] {
        let (f, path, request, proof) = authorized().await;
        let claim = f
            .store
            .claim_model_review(&f.owner, &request)
            .await
            .unwrap()
            .unwrap();
        assert!(f.store.begin_model_review(&claim, &proof).await.unwrap());
        match mode {
            "cancel" => {
                f.store
                    .cancel_model_authorization(&f.owner, &request)
                    .await
                    .unwrap();
            }
            "evidence" => {
                let plan = f
                    .call(
                        "GET",
                        &format!("/api/learning/plans/{}", claim.authorization.plan_id),
                        json!({}),
                    )
                    .await
                    .1;
                let evidence = &plan["results"][0]["evidence"]["request_id"];
                f.call("DELETE", &path, json!({"request_id":evidence}))
                    .await;
            }
            _ => {
                f.store
                    .revoke_subscription_connection(&f.owner, &claim.authorization.connection_id, 1)
                    .await
                    .unwrap();
            }
        }
        let result = f
            .store
            .finish_model_review(&claim, Some(output(&claim.authorization)))
            .await
            .unwrap();
        assert_eq!(
            result.status,
            if mode == "cancel" {
                "cancelled"
            } else {
                "invalidated"
            }
        );
        assert!(result.advice.is_none());
        assert!(result.preview.is_none());
        assert!(
            f.store
                .claim_model_review(&f.owner, &request)
                .await
                .unwrap()
                .is_none()
        );
        f.cleanup().await;
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_execution_rejects_untrusted_outputs_and_recovers_abandoned_claims_without_resend()
{
    for mode in ["invalid", "unsent", "deadline", "runtime_failure"] {
        let (f, _, request, proof) = authorized().await;
        let result = if mode == "runtime_failure" {
            execute_model_review(
                f.store.as_ref(),
                &Runtime {
                    proof: proof.clone(),
                    calls: AtomicUsize::new(0),
                    fail: true,
                },
                &f.owner,
                &request,
            )
            .await
            .unwrap()
            .unwrap()
        } else {
            let claim = f
                .store
                .claim_model_review(&f.owner, &request)
                .await
                .unwrap()
                .unwrap();
            if mode != "unsent" {
                f.store.begin_model_review(&claim, &proof).await.unwrap();
            }
            if mode == "deadline" {
                sqlx::query("UPDATE learning_model_authorizations SET dispatch_deadline_ms=0 WHERE user_id=$1").bind(Uuid::parse_str(f.owner.as_str()).unwrap()).execute(&f.pool).await.unwrap();
            }
            let bytes = if mode == "invalid" {
                br#"{"score":100,"command":"execute"}"#.to_vec()
            } else {
                output(&claim.authorization)
            };
            f.store
                .finish_model_review(&claim, Some(bytes))
                .await
                .unwrap()
        };
        assert_eq!(result.status, "unknown", "{mode}");
        assert!(result.advice.is_none());
        assert!(result.preview.is_none());
        assert!(
            f.store
                .claim_model_review(&f.owner, &request)
                .await
                .unwrap()
                .is_none()
        );
        f.cleanup().await;
    }
}
#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_completed_advice_is_cleared_with_evidence_and_failed_finish_never_reclaims() {
    let (f, path, request, proof) = authorized().await;
    let claim = f
        .store
        .claim_model_review(&f.owner, &request)
        .await
        .unwrap()
        .unwrap();
    f.store.begin_model_review(&claim, &proof).await.unwrap();
    let constraint = format!("fail_learning_finish_{}", Uuid::new_v4().simple());
    let owner = Uuid::parse_str(f.owner.as_str()).unwrap();
    sqlx::query(&format!("ALTER TABLE learning_model_authorization_audit ADD CONSTRAINT {constraint} CHECK(user_id<>'{owner}'::uuid OR event<>'succeeded')")).execute(&f.pool).await.unwrap();
    let result = f
        .store
        .finish_model_review(&claim, Some(output(&claim.authorization)))
        .await;
    sqlx::query(&format!(
        "ALTER TABLE learning_model_authorization_audit DROP CONSTRAINT {constraint}"
    ))
    .execute(&f.pool)
    .await
    .unwrap();
    assert!(result.is_err());
    assert!(!f.store.begin_model_review(&claim, &proof).await.unwrap());
    assert!(
        f.store
            .claim_model_review(&f.owner, &request)
            .await
            .unwrap()
            .is_none()
    );
    // Retrying persistence of an already received reply never calls the model again.
    f.store
        .finish_model_review(&claim, Some(output(&claim.authorization)))
        .await
        .unwrap();
    let plan = f
        .call(
            "GET",
            &format!("/api/learning/plans/{}", claim.authorization.plan_id),
            json!({}),
        )
        .await
        .1;
    f.call(
        "DELETE",
        &path,
        json!({"request_id":plan["results"][0]["evidence"]["request_id"]}),
    )
    .await;
    let item = f
        .store
        .get_model_authorization(&f.owner, &request)
        .await
        .unwrap();
    assert_eq!(item.status, "invalidated");
    assert!(item.advice.is_none());
    let body: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT advice FROM learning_model_authorizations WHERE user_id=$1")
            .bind(owner)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(body.is_none());
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_model_to_human_confirmation_and_source_erasure_is_a_complete_loop() {
    use personal_ai_storage::learning_operations::LearningOperationsStore;
    let (f, path, request, proof) = authorized().await;
    let runtime = Runtime {
        proof,
        calls: AtomicUsize::new(0),
        fail: false,
    };
    let before = f.call("GET", "/api/learning/snapshot", json!({})).await.1;
    let report = f.store.audit_learning_models(&f.owner, None).await.unwrap();
    assert!(report.consistent, "{:?}", report.issues);
    assert_eq!(report.counts["authorized"], 1);
    let item = execute_model_review(f.store.as_ref(), &runtime, &f.owner, &request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.status, "succeeded");
    let report = f.store.audit_learning_models(&f.owner, None).await.unwrap();
    assert!(report.consistent, "{:?}", report.issues);
    assert_eq!(report.counts["succeeded"], 1);
    assert_eq!(
        f.call("GET", "/api/learning/snapshot", json!({})).await.1,
        before
    );
    let plan_path = format!("/api/learning/plans/{}", item.plan_id);
    let plan = f.call("GET", &plan_path, json!({})).await.1;
    assert!(plan["results"][0]["evidence"]["review"].is_null());
    let evidence = plan["results"][0]["evidence"]["request_id"].clone();
    let review = Uuid::new_v4();
    let dim = json!({"verdict":"supported","reason":"人工检查了实际产物和复现记录"});
    let review_path = format!("{path}/review");
    let (status, _) = f.call("POST", &review_path, json!({"request_id":review,"evidence_request_id":evidence,"body":{"explanation":dim,"work":dim,"verification":dim,"limitations":dim}})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        f.call("GET", "/api/learning/snapshot", json!({})).await.1,
        before
    );
    let confirmation = json!({"request_id":Uuid::new_v4(),"review_request_id":review,"expected_revision":"1","score":65});
    let confirm_path = format!("{review_path}/confirm");
    assert_eq!(
        f.call("POST", &confirm_path, confirmation.clone()).await.0,
        StatusCode::OK
    );
    let snapshot = f.call("GET", "/api/learning/snapshot", json!({})).await.1;
    assert_eq!(snapshot["revision"], "2");
    assert_eq!(snapshot["assessments"][0]["score"], 65);
    let (status, erased) = f
        .call("DELETE", &path, json!({"request_id":evidence}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(erased["results"][0]["note"], plan["results"][0]["note"]);
    assert!(erased["results"][0]["evidence"]["review"]["confirmed_score"].is_null());
    let snapshot = f.call("GET", "/api/learning/snapshot", json!({})).await.1;
    assert_eq!(snapshot["revision"], "3");
    assert_eq!(snapshot["assessments"], json!([]));
    assert_eq!(
        f.call("POST", &confirm_path, confirmation).await.0,
        StatusCode::CONFLICT
    );
    let item = f
        .store
        .get_model_authorization(&f.owner, &request)
        .await
        .unwrap();
    assert_eq!(item.status, "invalidated");
    assert!(item.advice.is_none());
    assert!(
        execute_model_review(f.store.as_ref(), &runtime, &f.owner, &request)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
    let report = f.store.audit_learning_models(&f.owner, None).await.unwrap();
    assert!(report.consistent, "{:?}", report.issues);
    assert_eq!(report.counts["invalidated"], 1);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "需要一次性 TEST_DATABASE_URL"]
async fn learning_model_audit_warns_about_elapsed_dispatch_without_mutating_it() {
    use personal_ai_storage::learning_operations::LearningOperationsStore;
    let (f, _, request, _) = authorized().await;
    f.store
        .claim_model_review(&f.owner, &request)
        .await
        .unwrap()
        .unwrap();
    let owner = Uuid::parse_str(f.owner.as_str()).unwrap();
    sqlx::query("UPDATE learning_model_authorizations SET created_ms=0,expires_ms=300000,approved_ms=1,dispatch_deadline_ms=2 WHERE user_id=$1").bind(owner).execute(&f.pool).await.unwrap();
    let report = f.store.audit_learning_models(&f.owner, None).await.unwrap();
    assert!(report.consistent, "{:?}", report.issues);
    assert_eq!(report.warnings, ["dispatch_deadline_elapsed"]);
    let status: String =
        sqlx::query_scalar("SELECT status FROM learning_model_authorizations WHERE user_id=$1")
            .bind(owner)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(status, "running");
    // The ordinary business read performs cleanup, unlike the operations read.
    assert_eq!(
        f.store
            .get_model_authorization(&f.owner, &request)
            .await
            .unwrap()
            .status,
        "unknown"
    );
    assert!(
        f.store
            .claim_model_review(&f.owner, &request)
            .await
            .unwrap()
            .is_none()
    );
    f.cleanup().await;
}

#[path = "model_execution/progress.rs"]
mod progress;

#[path = "model_execution/events.rs"]
mod events;

#[path = "model_execution/text_events.rs"]
mod text_events;
