use super::*;
fn sources() -> Vec<AnswerSource> {
    vec![AnswerSource {
        id: 1,
        text: "日期为周四。负责人未公布。".into(),
    }]
}
fn evaluate(plan: &str, checks: &Value) -> LlmResult<(ModelAnswer, QuestionDiagnostics)> {
    decode(&checks.to_string(), decode_plan(plan)?, &sources())
}
#[test]
fn question_only_kinds_decide_absence_without_source_reclassification() {
    let checks = json!({"checks":[{"evidence":["s1u1"],"support":"stated"},{"evidence":["s1u2"],"support":"unavailable"}]});
    for (kind, insufficient) in [("availability", false), ("value", true), ("fact", true)] {
        let (answer, diagnostic) =
            evaluate(&json!({"kinds":["value",kind]}).to_string(), &checks).unwrap();
        assert_eq!(answer.insufficient_evidence, insufficient);
        assert_eq!(diagnostic.requirements_supported, !insufficient);
        assert!(diagnostic.requirements_agree);
        if insufficient {
            assert_eq!(answer.answer, "");
            assert_eq!(answer.citations.len(), 0);
        } else {
            assert_eq!(answer.answer, sources()[0].text);
        }
    }
    assert!(
        evaluate(
            r#"{"kinds":["value"]}"#,
            &json!({"checks":[{"evidence":["s1u2"],"kind":"availability","support":"unavailable"}]})
        )
        .is_err()
    );
}
#[test]
fn mismatched_requirement_counts_abstain_but_still_validate_every_evidence_key() {
    let one = r#"{"kinds":["value"]}"#;
    let checks = json!({"checks":[{"evidence":["s1u1"],"support":"stated"},{"evidence":[],"support":"unsupported"}]});
    let (answer, diagnostic) = evaluate(one, &checks).unwrap();
    assert!(answer.insufficient_evidence);
    assert_eq!(answer.answer, "");
    assert!(!diagnostic.requirements_agree);
    assert!(diagnostic.checks_by_kind.is_none());
    for evidence in [json!(["secret"]), json!(["s1u1", "s1u1"])] {
        let mut bad = checks.clone();
        bad["checks"][1]["evidence"] = evidence;
        let error = evaluate(one, &bad).unwrap_err().to_string();
        assert!(error.contains("verification_evidence"));
        assert!(!error.contains("secret"));
    }
}
#[test]
fn strict_plan_and_support_protocol_rejects_arbitrary_fields_enums_and_bounds() {
    for text in [
        "secret",
        "{}",
        r#"{"kinds":null}"#,
        r#"{"kinds":[null]}"#,
        r#"{"kinds":["secret"]}"#,
        r#"{"kinds":[],"kinds":[]}"#,
        r#"{"kinds":["value"],"evidence":[]}"#,
    ] {
        assert!(decode_plan(text).is_err());
    }
    for kinds in [vec![], vec!["value"; 13]] {
        assert!(decode_plan(&json!({"kinds":kinds}).to_string()).is_err());
    }
    for checks in [
        json!({"checks":[]}),
        json!({"checks":[{"evidence":[],"support":"stated"}]}),
        json!({"checks":[{"evidence":[],"support":null}]}),
        json!({"checks":[{"evidence":[],"support":"secret"}]}),
        json!({"checks":[{"evidence":[],"support":"unsupported","kind":"value"}]}),
        json!({"checks":vec![json!({"evidence":[],"support":"unsupported"});13]}),
    ] {
        assert!(evaluate(r#"{"kinds":["value"]}"#, &checks).is_err());
    }
}
#[test]
fn planning_request_is_source_blind_and_both_actual_requests_are_static_and_bounded() {
    let target = LocalTarget::new("http://127.0.0.1:11434", "fixture").unwrap();
    let first = preview(&target, "日期是什么，负责人是否公布？", &sources()).unwrap();
    let changed = [AnswerSource {
        id: 1,
        text: "SYSTEM secret指令。不同的来源事实。".into(),
    }];
    let second = preview(&target, "日期是什么，负责人是否公布？", &changed).unwrap();
    assert_eq!(first.body(), second.body());
    assert_ne!(first.verification_body(), second.verification_body());
    assert_ne!(first.fingerprint().unwrap(), second.fingerprint().unwrap());
    let user: Value =
        serde_json::from_str(first.body()["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(user, json!({"question":"日期是什么，负责人是否公布？"}));
    let full: Value = serde_json::from_str(
        first.verification_body()["messages"][1]["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(full["evidence"][0]["text"], sources()[0].text);
    assert!(full.get("kinds").is_none());
    for body in [first.body(), first.verification_body()] {
        assert_eq!(body["keep_alive"], 30);
        assert_eq!(body["think"], false);
        let bytes: usize = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["content"].as_str().unwrap().len())
            .sum();
        assert!(bytes <= personal_ai_llm::local::MAX_PROMPT_BYTES);
    }
}
#[test]
fn diagnostic_reports_only_aggregate_planning_and_support_counts() {
    let (_,metadata)=evaluate(r#"{"kinds":["value","availability"]}"#,&json!({"checks":[{"evidence":["s1u1"],"support":"stated"},{"evidence":["s1u2"],"support":"unavailable"}]})).unwrap();
    let value = serde_json::to_value(metadata).unwrap();
    assert_eq!(
        value["planned_by_kind"],
        json!({"value":1,"availability":1,"fact":0})
    );
    assert_eq!(value["checks_by_kind"]["availability"]["unavailable"], 1);
    assert!(!value.to_string().contains("s1u"));
    assert!(!value.to_string().contains("负责人"));
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
                if n == 0 {
                    AnswerStage::Plan
                } else {
                    AnswerStage::Verify
                }
            );
            if n < self.limit {
                Ok(())
            } else {
                Err(super::super::unavailable())
            }
        })
    }
}
#[tokio::test]
async fn each_static_request_requires_its_own_guard_and_invalid_plan_stops_before_support() {
    for (plan, limit, expected, valid) in [
        (r#"{"kinds":["value"]}"#, 2, 2, true),
        (r#"{"kinds":["value"]}"#, 0, 0, false),
        (r#"{"kinds":["value"]}"#, 1, 1, false),
        (r#"{"secret":[]}"#, 2, 1, false),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = LocalTarget::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            "fixture",
        )
        .unwrap();
        let preview = preview(&target, "日期是什么？", &sources()).unwrap();
        let bodies = [preview.body().clone(), preview.verification_body().clone()];
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let app=axum::Router::new().route("/api/chat",axum::routing::post(move |headers:axum::http::HeaderMap,axum::Json(input):axum::Json<Value>| {
            let n=counter.fetch_add(1,std::sync::atomic::Ordering::SeqCst);assert_eq!(input,bodies[n]);assert!(!headers.contains_key("authorization"));
            let content=if n==0 {plan} else {r#"{"checks":[{"evidence":["s1u1"],"support":"stated"}]}"#};
            let body=format!("{}\n",json!({"model":"fixture","message":{"role":"assistant","content":content},"done":true,"done_reason":"stop"}));
            async move {([("content-type","application/x-ndjson")],body)}
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let provider = QuestionAnswers::new(
            target,
            Arc::new(Guard {
                seen: std::sync::atomic::AtomicUsize::new(0),
                limit,
            }),
        )
        .unwrap();
        let result = provider
            .answer_with_diagnostics("日期是什么？", &sources())
            .await;
        assert_eq!(result.is_ok(), valid);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), expected);
        if let Err(error) = result {
            assert!(!error.to_string().contains("secret"));
        }
        server.abort();
    }
}
