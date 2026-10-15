use super::*;

#[test]
fn only_verifier_schema_and_its_declaration_change_while_all_original_inputs_remain_exact() {
    let target = LocalTarget::new("http://127.0.0.1:11434", "fixture").unwrap();
    let question = "负责人是否公布？保留🙂 e\u{301}、引号\"和换行\n。";
    let sources = vec![AnswerSource {
        id: 1,
        text: "活动在周四举行。\n🙂 e\u{301}".into(),
    }];
    let original = serde_json::to_value(
        super::super::question_first::preview(&target, question, &sources).unwrap(),
    )
    .unwrap();
    let result = preview(&target, question, &sources).unwrap();
    let mut actual = serde_json::to_value(&result).unwrap();
    assert_eq!(actual["profile"], PROFILE);
    assert_ne!(
        result.fingerprint().unwrap(),
        super::super::question_first::preview(&target, question, &sources)
            .unwrap()
            .fingerprint()
            .unwrap()
    );
    let schema = &actual["verification_body"]["format"];
    let branches = schema["properties"]["checks"]["items"]["anyOf"]
        .as_array()
        .unwrap();
    assert_eq!(branches.len(), 2);
    let original_item = &original["verification_body"]["format"]["properties"]["checks"]["items"];
    for (index, branch) in branches.iter().enumerate() {
        let minimum = i32::from(index == 0);
        let supports = if index == 0 {
            json!(["stated", "unavailable"])
        } else {
            json!(["unsupported"])
        };
        assert_eq!(branch["properties"]["evidence"]["minItems"], minimum);
        assert_eq!(branch["properties"]["support"]["enum"], supports);
        let mut restored = branch.clone();
        restored["properties"]["evidence"]["minItems"] = json!(0);
        restored["properties"]["support"]["enum"] = json!(["stated", "unavailable", "unsupported"]);
        assert_eq!(&restored, original_item);
    }
    assert_eq!(
        actual["verification_body"]["messages"][0]["content"],
        format!(
            "{}{schema}",
            super::super::super::contract::VERIFY_INSTRUCTION
        )
    );
    let mut restored_schema = schema.clone();
    restored_schema["properties"]["checks"]["items"] = original_item.clone();
    assert_eq!(restored_schema, original["verification_body"]["format"]);
    actual["profile"] = original["profile"].clone();
    actual["verification_body"]["format"] = original["verification_body"]["format"].clone();
    actual["verification_body"]["messages"][0]["content"] =
        original["verification_body"]["messages"][0]["content"].clone();
    assert_eq!(actual, original);
    assert!(preview(&target, " ", &sources).is_err());
    assert!(preview(&target, question, &[]).is_err());
    assert!(preview(&target, &"问".repeat(1001), &sources).is_err());
}

#[tokio::test]
async fn actual_requests_match_preview_and_decoder_rejects_empty_supported_evidence_even_if_schema_is_ignored()
 {
    use super::super::AnswerStage;
    struct Guard;
    impl NativeStageGuard for Guard {
        fn permit(&self, _: AnswerStage) -> BoxFuture<'_, LlmResult<()>> {
            Box::pin(async { Ok(()) })
        }
    }
    for support in ["stated", "unavailable", "unsupported"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = LocalTarget::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            "fixture",
        )
        .unwrap();
        let sources = vec![AnswerSource {
            id: 1,
            text: "活动在周四举行。".into(),
        }];
        let question = "负责人是否公布？";
        let expected = preview(&target, question, &sources).unwrap();
        let bodies = [
            expected.planning_body,
            expected.selection_body,
            expected.verification_body,
        ];
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = calls.clone();
        let app = axum::Router::new().route("/api/chat", axum::routing::post(move |axum::Json(input): axum::Json<Value>| {
            let index = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(input, bodies[index]);
            let content = match index {
                0 => json!({"kinds":["availability"]}),
                1 => json!({"evidence":[]}),
                _ => json!({"checks":[{"evidence":[],"kind":"availability","support":support}]}),
            }.to_string();
            let body = format!("{}\n", json!({"model":"fixture","message":{"role":"assistant","content":content},"done":true,"done_reason":"stop"}));
            async move { ([("content-type","application/x-ndjson")], body) }
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let result = EvidenceBoundAnswers::new(target, Arc::new(Guard))
            .unwrap()
            .answer_with_diagnostics(question, &sources)
            .await;
        if support == "unsupported" {
            assert!(result.unwrap().0.insufficient_evidence);
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("verification_decision")
            );
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
        server.abort();
    }
}
