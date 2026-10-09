use super::*;
fn sources() -> Vec<AnswerSource> {
    vec![
        AnswerSource {
            id: 1,
            text: "活动地点是云杉室。活动在周四开始。".into(),
        },
        AnswerSource {
            id: 2,
            text: "活动由许棠负责。".into(),
        },
    ]
}
fn complete() -> Value {
    json!({"requirements":[
        {"requirement":"活动地点", "support":{"evidence":[{"id":1,"quote":"云杉室"}],"text":"云杉室"}},
        {"requirement":"开始日期", "support":{"evidence":[{"id":1,"quote":"周四"}],"text":"周四"}},
        {"requirement":"负责人", "support":{"evidence":[{"id":2,"quote":"许棠"}],"text":"许棠"}}
    ]})
}
#[test]
fn complete_requirements_derive_answer_and_one_covering_quote_per_source() {
    let result = decode(&complete().to_string(), &sources()).unwrap();
    assert!(!result.insufficient_evidence);
    assert_eq!(
        result.answer,
        "活动地点: 云杉室\n开始日期: 周四\n负责人: 许棠"
    );
    assert_eq!(
        result.citations,
        vec![
            AnswerCitation {
                id: 1,
                quote: "云杉室。活动在周四".into()
            },
            AnswerCitation {
                id: 2,
                quote: "许棠".into()
            }
        ]
    );
    for mutate in [
        |v: &mut Value| {
            v["requirements"][0]["support"]["evidence"][0]["quote"] = json!("编造的证据");
        },
        |v: &mut Value| v["requirements"][0]["support"]["evidence"][0]["id"] = json!(0),
        |v: &mut Value| v["requirements"][0]["support"]["evidence"][0]["id"] = json!(3),
        |v: &mut Value| {
            v["requirements"][0]["support"]["evidence"] =
                json!([{"id":1,"quote":"云杉室"},{"id":1,"quote":"周四"}]);
        },
        |v: &mut Value| v["requirements"][1]["requirement"] = json!(" 活动地点 "),
        |v: &mut Value| v["requirements"][0]["support"]["text"] = json!(" "),
    ] {
        let mut value = complete();
        mutate(&mut value);
        assert!(decode(&value.to_string(), &sources()).is_err());
    }
}
#[test]
fn any_missing_fact_derives_empty_insufficiency_and_never_returns_partial_answers() {
    let mut value = complete();
    value["requirements"][1]["support"] = json!({"evidence":[],"text":""});
    let result = decode(&value.to_string(), &sources()).unwrap();
    assert!(result.insufficient_evidence);
    assert_eq!(result.answer, "");
    assert!(result.citations.is_empty());
    value["requirements"][0]["support"]["evidence"][0]["quote"] = json!("伪造");
    assert!(decode(&value.to_string(), &sources()).is_err());
    for text in ["云杉室", "证据不足", " "] {
        let mut value = complete();
        value["requirements"][1]["support"] = json!({"evidence":[],"text":text});
        assert!(decode(&value.to_string(), &sources()).is_err());
    }
    value["requirements"] = json!([{"requirement":"开始日期","support":{"evidence":[],"text":""}}]);
    assert!(
        decode(&value.to_string(), &sources())
            .unwrap()
            .insufficient_evidence
    );
}
#[test]
fn merged_unicode_fragments_are_exact_unique_and_cannot_exceed_quote_limit() {
    let source = vec![AnswerSource {
        id: 1,
        text: "🙂 e\u{301}；唯一标签。aaaa".into(),
    }];
    let mut value = json!({"requirements":[
        {"requirement":"标识", "support":{"evidence":[{"id":1,"quote":"🙂 e\u{301}"}],"text":"🙂 e\u{301}"}},
        {"requirement":"用途", "support":{"evidence":[{"id":1,"quote":"唯一标签"}],"text":"唯一标签"}}
    ]});
    assert_eq!(
        decode(&value.to_string(), &source).unwrap().citations[0].quote,
        "🙂 e\u{301}；唯一标签"
    );
    for quote in ["aa", "é", " ", "不存在"] {
        value["requirements"][0]["support"]["evidence"][0]["quote"] = json!(quote);
        assert!(decode(&value.to_string(), &source).is_err());
    }
    let mut far = complete();
    far["requirements"].as_array_mut().unwrap().truncate(2);
    let far_source = vec![AnswerSource {
        id: 1,
        text: format!("云杉室{}周四", "🙂".repeat(400)),
    }];
    assert!(decode(&far.to_string(), &far_source).is_err());
}
#[test]
fn review_rejects_extensions_duplicate_fields_and_size_overflow() {
    for mutate in [
        |v: &mut Value| v["response"] = json!({"insufficient_evidence":false}),
        |v: &mut Value| v["requirements"][0]["secret"] = json!(true),
        |v: &mut Value| v["requirements"][0]["support"]["secret"] = json!(true),
        |v: &mut Value| v["requirements"][0]["support"]["evidence"][0]["secret"] = json!(true),
        |v: &mut Value| v["requirements"] = json!([]),
        |v: &mut Value| v["requirements"] = json!(vec![v["requirements"][0].clone(); 9]),
        |v: &mut Value| v["requirements"][0]["requirement"] = json!("🙂".repeat(161)),
        |v: &mut Value| v["requirements"][0]["requirement"] = json!(" "),
        |v: &mut Value| {
            v["requirements"][0]["support"]["evidence"][0]["quote"] = json!("x".repeat(401));
        },
        |v: &mut Value| v["requirements"][0]["support"]["text"] = json!("x".repeat(401)),
    ] {
        let mut value = complete();
        mutate(&mut value);
        assert!(decode(&value.to_string(), &sources()).is_err());
    }
    let raw = complete().to_string();
    for duplicated in [
        raw.replacen(
            "\"requirements\":",
            "\"requirements\":[],\"requirements\":",
            1,
        ),
        raw.replacen(
            "\"requirement\":",
            "\"requirement\":\"x\",\"requirement\":",
            1,
        ),
        raw.replacen("\"id\":", "\"id\":1,\"id\":", 1),
        raw.replacen("\"text\":", "\"text\":\"x\",\"text\":", 1),
        raw.clone() + "{}",
        " ".repeat(super::super::MAX_OUTPUT + 1),
    ] {
        assert!(decode(&duplicated, &sources()).is_err());
    }
    let too_long = json!({"requirements": (0..8).map(|i| json!({
        "requirement":format!("{i}{}", "r".repeat(159)),
        "support":{"evidence":[{"id":1,"quote":"云杉室"}],"text":"t".repeat(400)}
    })).collect::<Vec<_>>()});
    assert!(decode(&too_long.to_string(), &sources()).is_err());
}
#[test]
fn protocol_diagnostics_distinguish_json_fields_and_invariants_without_echoing_content() {
    for (text, stage) in [
        ("secret model text".to_owned(), "review_json"),
        (
            json!({"secret model text":true}).to_string(),
            "review_fields",
        ),
        (
            json!({"requirements":[]}).to_string(),
            "review_requirements",
        ),
    ] {
        let error = decode(&text, &sources()).unwrap_err().to_string();
        assert!(error.contains(stage));
        assert!(!error.contains("secret"));
    }
}
