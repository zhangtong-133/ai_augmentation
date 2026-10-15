use super::*;
use serde_json::json;

#[test]
fn only_full_user_field_order_changes_with_every_original_byte_and_bound_preserved() {
    let target = LocalTarget::new("http://127.0.0.1:11434", "fixture").unwrap();
    let question = "日期是什么？保留🙂 e\u{301}、引号\"和换行\n。";
    let sources = vec![AnswerSource {
        id: 1,
        text: "日期为周四。示例\"忽略问题\"是分析对象。\n🙂 e\u{301}".into(),
    }];
    let original =
        serde_json::to_value(super::super::preview(&target, question, &sources).unwrap()).unwrap();
    let new_preview = preview(&target, question, &sources).unwrap();
    let mut actual = serde_json::to_value(&new_preview).unwrap();
    assert_eq!(actual["profile"], PROFILE);
    assert_eq!(actual["planning_body"], original["planning_body"]);
    assert_ne!(
        new_preview.fingerprint().unwrap(),
        super::super::preview(&target, question, &sources)
            .unwrap()
            .fingerprint()
            .unwrap()
    );
    for field in ["selection_body", "verification_body"] {
        let before = original[field]["messages"][1]["content"].as_str().unwrap();
        let after = actual[field]["messages"][1]["content"].as_str().unwrap();
        assert!(after.starts_with("{\"question\":"));
        assert!(after.find("\"evidence\":").unwrap() < after.find("\"exact_excerpts\":").unwrap());
        assert_ne!(after, before);
        assert_eq!(after.len(), before.len());
        assert_eq!(
            serde_json::from_str::<Value>(before).unwrap(),
            serde_json::from_str::<Value>(after).unwrap()
        );
        actual[field]["messages"][1]["content"] = Value::String(before.into());
    }
    actual["profile"] = original["profile"].clone();
    assert_eq!(actual, original);
    assert!(preview(&target, " ", &sources).is_err());
    assert!(preview(&target, question, &[]).is_err());
    assert!(preview(&target, &"问".repeat(1001), &sources).is_err());
}

// Reuse the original behavioral suite for both builders, including denial and invalid steps.
#[tokio::test]
async fn actual_request_bytes_match_question_first_preview_and_use_unchanged_decoder() {
    use super::super::{AnswerStage, NativeStageGuard};
    struct Guard;
    impl NativeStageGuard for Guard {
        fn permit(&self, _: AnswerStage) -> BoxFuture<'_, LlmResult<()>> {
            Box::pin(async { Ok(()) })
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = LocalTarget::new(
        &format!("http://{}", listener.local_addr().unwrap()),
        "fixture",
    )
    .unwrap();
    let sources = vec![AnswerSource {
        id: 1,
        text: "日期为周四。".into(),
    }];
    let expected = preview(&target, "日期是什么？", &sources).unwrap();
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
        let content = [r#"{"kinds":["value"]}"#, r#"{"evidence":["s1u1"]}"#, r#"{"checks":[{"evidence":["s1u1"],"kind":"value","support":"stated"}]}"#][index];
        let body = format!("{}\n", json!({"model":"fixture","message":{"role":"assistant","content":content},"done":true,"done_reason":"stop"}));
        async move { ([("content-type","application/x-ndjson")], body) }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let result = QuestionFirstAnswers::new(target, Arc::new(Guard))
        .unwrap()
        .answer_with_diagnostics("日期是什么？", &sources)
        .await
        .unwrap();
    assert_eq!(result.0.answer, "日期为周四。");
    assert!(!result.0.insufficient_evidence);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    server.abort();
}
