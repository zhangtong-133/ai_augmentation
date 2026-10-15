fn sources() -> Vec<AnswerSource> {
    vec![AnswerSource {
        id: 1,
        text: "日期为周四。负责人未公布。".into(),
    }]
}
use super::*;
fn evaluate(
    kinds: Value,
    evidence: Value,
    checks: Value,
) -> LlmResult<(ModelAnswer, GuardedDiagnostics)> {
    let plan = Value::Object([("kinds".into(), kinds)].into_iter().collect());
    let selection = Value::Object([("evidence".into(), evidence)].into_iter().collect());
    let verification = Value::Object([("checks".into(), checks)].into_iter().collect());
    let sources = sources();
    let selected = contract::decode_selection(
        &selection.to_string(),
        &sources,
        &contract::catalog(&sources)?,
    )?;
    decode(
        &verification.to_string(),
        decode_plan(&plan.to_string())?,
        &selected,
        &sources,
    )
}
#[test]
fn source_blind_types_and_independent_evidence_must_both_support_the_answer() {
    let checks = json!([{"evidence":["s1u2"],"kind":"value","support":"unavailable"}]);
    let (answer, meta) =
        evaluate(json!(["availability"]), json!(["s1u2"]), checks.clone()).unwrap();
    assert!(!answer.insufficient_evidence);
    assert_eq!(answer.answer, "负责人未公布。");
    assert_eq!(meta.verifier_kind_disagreements, Some(1));
    assert!(meta.evidence_agrees);
    for (kinds, selected) in [
        (json!(["value"]), json!(["s1u2"])),
        (json!(["availability"]), json!(["s1u1"])),
    ] {
        let (answer, _) = evaluate(kinds, selected, checks.clone()).unwrap();
        assert!(answer.insufficient_evidence);
        assert!(answer.answer.is_empty() && answer.citations.is_empty());
    }
}
#[test]
fn count_mismatch_abstains_and_never_bypasses_strict_verification() {
    let check = json!({"evidence":["s1u1"],"kind":"value","support":"stated"});
    let (answer, meta) =
        evaluate(json!(["value", "value"]), json!(["s1u1"]), json!([check])).unwrap();
    assert!(answer.insufficient_evidence);
    assert_eq!(meta.verifier_kind_disagreements, None);
    assert!(!meta.question.requirements_agree);
    for checks in [
        json!([]),
        json!([{"evidence":["secret"],"kind":"value","support":"stated"}]),
        json!([{"evidence":["s1u1"],"kind":null,"support":"stated"}]),
        json!([{"evidence":["s1u1"],"kind":"value","support":"stated","extra":"secret"}]),
    ] {
        let error = evaluate(json!(["value", "value"]), json!(["s1u1"]), checks)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret"));
    }
}
#[test]
fn all_three_static_bodies_exactly_reuse_the_frozen_candidate_components() {
    let target = LocalTarget::new("http://127.0.0.1:11434", "fixture").unwrap();
    let sources = sources();
    let question = "日期是什么，负责人是否公布？";
    let actual = preview(&target, question, &sources).unwrap();
    let old_question = super::super::preview(&target, question, &sources).unwrap();
    let old_evidence =
        super::super::super::preview_with_residency(&target, question, &sources, Residency::Reuse)
            .unwrap();
    assert_eq!(&actual.planning_body, old_question.body());
    assert_eq!(&actual.selection_body, old_evidence.body());
    assert_eq!(&actual.verification_body, old_evidence.verification_body());
    let (_, meta) = evaluate(
        json!(["availability"]),
        json!(["s1u2"]),
        json!([{"evidence":["s1u2"],"kind":"value","support":"unavailable"}]),
    )
    .unwrap();
    let text = serde_json::to_string(&meta).unwrap();
    assert!(!text.contains("s1u") && !text.contains("负责人"));
}
struct Guard {
    seen: std::sync::atomic::AtomicUsize,
    limit: usize,
}
impl NativeStageGuard for Guard {
    fn permit(&self, stage: AnswerStage) -> BoxFuture<'_, LlmResult<()>> {
        Box::pin(async move {
            let n = self.seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(
                stage,
                [AnswerStage::Plan, AnswerStage::Select, AnswerStage::Verify][n]
            );
            if n < self.limit {
                Ok(())
            } else {
                Err(super::super::super::unavailable())
            }
        })
    }
}
#[tokio::test]
async fn three_fresh_guards_and_early_protocol_validation_bound_native_requests() {
    for (plan, selection, limit, expected, valid) in [
        (
            r#"{"kinds":["value"]}"#,
            r#"{"evidence":["s1u1"]}"#,
            3,
            3,
            true,
        ),
        (
            r#"{"kinds":["value"]}"#,
            r#"{"evidence":["s1u1"]}"#,
            0,
            0,
            false,
        ),
        (
            r#"{"kinds":["value"]}"#,
            r#"{"evidence":["s1u1"]}"#,
            1,
            1,
            false,
        ),
        (
            r#"{"kinds":["value"]}"#,
            r#"{"evidence":["s1u1"]}"#,
            2,
            2,
            false,
        ),
        (r#"{"secret":[]}"#, r#"{"evidence":["s1u1"]}"#, 3, 1, false),
        (
            r#"{"kinds":["value"]}"#,
            r#"{"evidence":["secret"]}"#,
            3,
            2,
            false,
        ),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = LocalTarget::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            "fixture",
        )
        .unwrap();
        let preview = preview(&target, "日期是什么？", &sources()).unwrap();
        let bodies = [
            preview.planning_body,
            preview.selection_body,
            preview.verification_body,
        ];
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let app = axum::Router::new().route("/api/chat",axum::routing::post(move |headers:axum::http::HeaderMap,axum::Json(input):axum::Json<Value>| {
            let n = counter.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
            assert_eq!(input,bodies[n]); assert!(!headers.contains_key("authorization"));
            let content = match n { 0=>plan,1=>selection,_=>r#"{"checks":[{"evidence":["s1u1"],"kind":"value","support":"stated"}]}"# };
            let body = format!("{}\n",json!({"model":"fixture","message":{"role":"assistant","content":content},"done":true,"done_reason":"stop"}));
            async move { ([("content-type","application/x-ndjson")],body) }
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let result = GuardedAnswers::new(
            target,
            Arc::new(Guard {
                seen: std::sync::atomic::AtomicUsize::new(0),
                limit,
            }),
        )
        .unwrap()
        .answer_with_diagnostics("日期是什么？", &sources())
        .await;
        assert_eq!(result.is_ok(), valid);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), expected);
        server.abort();
    }
}
