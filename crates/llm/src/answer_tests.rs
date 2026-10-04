use super::*;
fn sources() -> Vec<AnswerSource> {
    vec![
        AnswerSource {
            id: 1,
            text: "前🙂言：逐字证据 e\u{301}\n结尾".into(),
        },
        AnswerSource {
            id: 2,
            text: "另一条资料".into(),
        },
    ]
}
#[test]
fn preview_and_actual_messages_preserve_exact_question_and_evidence() {
    let question = "  问题\n🙂  ";
    let prompt = prepare(question, &sources()).unwrap();
    let request = prompt.request();
    assert_eq!(request.messages.len(), 2);
    assert_eq!(request.messages[0].role, Role::System);
    assert_eq!(request.messages[0].content, prompt.system());
    assert_eq!(request.messages[1].role, Role::User);
    assert_eq!(request.messages[1].content, prompt.user());
    let user: Value = serde_json::from_str(prompt.user()).unwrap();
    assert_eq!(user["question"], question);
    assert_eq!(user["evidence"][0]["text"], sources()[0].text);
    assert_eq!(
        prompt.schema()["properties"]["citations"]["items"]["properties"]["id"]["enum"],
        json!([1, 2])
    );
    assert_eq!(
        prompt.fingerprint().unwrap(),
        prepare(question, &sources())
            .unwrap()
            .fingerprint()
            .unwrap()
    );
    let original = prompt.fingerprint().unwrap();
    for changed in ["问题\n🙂", "  问题\n🙂   "] {
        assert_ne!(
            original,
            prepare(changed, &sources()).unwrap().fingerprint().unwrap()
        );
    }
    let mut changed = sources();
    changed[0].text.push(' ');
    assert_ne!(
        original,
        prepare(question, &changed).unwrap().fingerprint().unwrap()
    );
    changed.swap(0, 1);
    changed[0].id = 1;
    changed[1].id = 2;
    assert_ne!(
        original,
        prepare(question, &changed).unwrap().fingerprint().unwrap()
    );
}
#[test]
fn request_limits_use_unicode_scalars_and_reject_invalid_source_numbering() {
    assert!(
        prepare(
            &"🙂".repeat(1000),
            &[AnswerSource {
                id: 1,
                text: "证".repeat(1000)
            }]
        )
        .is_ok()
    );
    for question in [String::new(), " \n ".into(), "🙂".repeat(1001)] {
        assert!(prepare(&question, &sources()).is_err());
    }
    for bad in [
        vec![],
        vec![AnswerSource {
            id: 0,
            text: "a".into(),
        }],
        vec![AnswerSource {
            id: 2,
            text: "a".into(),
        }],
        (1..=6)
            .map(|id| AnswerSource {
                id,
                text: "a".into(),
            })
            .collect(),
        vec![AnswerSource {
            id: 1,
            text: "x".repeat(1001),
        }],
        vec![AnswerSource {
            id: 1,
            text: " \n".into(),
        }],
    ] {
        assert!(prepare("q", &bad).is_err());
    }
}
#[test]
fn completed_json_requires_exact_fields_and_never_echoes_untrusted_output() {
    let valid =
        r#"{"answer":"a","citations":[{"id":1,"quote":"q"}],"insufficient_evidence":false}"#;
    assert_eq!(decode(valid).unwrap().citations[0].quote, "q");
    for bad in [
        "secret",
        "```json\n{}\n```",
        r#"{"answer":"secret","citations":[1],"insufficient_evidence":false}"#,
        r#"{"answer":"secret","citations":[{"id":1,"quote":"q","title":"secret"}],"insufficient_evidence":false}"#,
        r#"{"answer":"a","answer":"secret","citations":[],"insufficient_evidence":false}"#,
    ] {
        let error = decode(bad).unwrap_err();
        assert!(!error.to_string().contains("secret"));
    }
    assert!(decode(&" ".repeat(128 * 1024 + 1)).is_err());
}
