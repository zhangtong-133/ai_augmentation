use super::*;

fn sources() -> Vec<AnswerSource> {
    vec![
        AnswerSource {
            id: 1,
            text: "活动地点是云杉室。".into(),
        },
        AnswerSource {
            id: 2,
            text: "活动在周四开始。".into(),
        },
    ]
}
fn complete() -> Value {
    json!({
        "requirements":[
            {"requirement":"活动地点", "evidence":[{"id":1,"quote":"云杉室"}]},
            {"requirement":"开始日期", "evidence":[{"id":2,"quote":"周四"}]}
        ],
        "response":{"answer":"周四在云杉室开始。", "citations":[{"id":1,"quote":"活动地点是云杉室。"},{"id":2,"quote":"活动在周四开始。"}], "insufficient_evidence":false}
    })
}
#[test]
fn every_listed_requirement_needs_unique_evidence_and_final_source_coverage() {
    let result = decode(&complete().to_string(), &sources()).unwrap();
    assert!(!result.insufficient_evidence);
    assert_eq!(result.citations.len(), 2);
    let mut same_source = complete();
    same_source["requirements"][1]["evidence"] = json!([{"id":1,"quote":"云杉室"}]);
    same_source["response"]["citations"] = json!([{"id":1,"quote":"活动地点是云杉室。"}]);
    assert!(decode(&same_source.to_string(), &sources()).is_ok());
    // Quotes establish location and consistency, not whether they semantically support a claim.
    for mutate in [
        |v: &mut Value| v["requirements"][1]["evidence"] = json!([]),
        |v: &mut Value| v["response"]["citations"] = json!([{"id":1,"quote":"云杉室"}]),
        |v: &mut Value| v["requirements"][0]["evidence"][0]["quote"] = json!("编造的证据"),
        |v: &mut Value| v["requirements"][0]["evidence"][0]["id"] = json!(0),
        |v: &mut Value| v["requirements"][0]["evidence"][0]["id"] = json!(3),
        |v: &mut Value| v["response"]["citations"][0]["quote"] = json!("编造的证据"),
        |v: &mut Value| {
            v["requirements"][0]["evidence"] =
                json!([{"id":1,"quote":"云杉室"},{"id":1,"quote":"云杉室"}]);
        },
        |v: &mut Value| {
            v["response"]["citations"] = json!([{"id":1,"quote":"云杉室"},{"id":1,"quote":"云杉室"},{"id":2,"quote":"周四"}]);
        },
        |v: &mut Value| v["requirements"][1]["requirement"] = json!(" 活动地点 "),
        |v: &mut Value| v["response"]["insufficient_evidence"] = json!(true),
        |v: &mut Value| v["response"]["answer"] = json!(" "),
    ] {
        let mut value = complete();
        mutate(&mut value);
        assert!(decode(&value.to_string(), &sources()).is_err());
    }
}
#[test]
fn a_missing_requirement_only_allows_an_empty_insufficient_response_without_repair() {
    let mut value = complete();
    value["requirements"][1]["evidence"] = json!([]);
    value["response"] = json!({"answer":"", "citations":[], "insufficient_evidence":true});
    assert!(
        decode(&value.to_string(), &sources())
            .unwrap()
            .insufficient_evidence
    );
    value["requirements"][0]["evidence"] = json!([]);
    assert!(decode(&value.to_string(), &sources()).is_ok());
    for response in [
        json!({"answer":"时间未提供", "citations":[], "insufficient_evidence":true}),
        json!({"answer":"", "citations":[{"id":1,"quote":"云杉室"}], "insufficient_evidence":true}),
        json!({"answer":"云杉室", "citations":[{"id":1,"quote":"云杉室"}], "insufficient_evidence":false}),
    ] {
        value["response"] = response;
        assert!(decode(&value.to_string(), &sources()).is_err());
    }
}
#[test]
fn review_rejects_ambiguous_unicode_quotes_extensions_duplicates_and_size_overflow() {
    let source = vec![AnswerSource {
        id: 1,
        text: "aaaa🙂 e\u{301}，唯一".into(),
    }];
    let mut value = json!({"requirements":[{"requirement":"标签", "evidence":[{"id":1,"quote":"🙂 e\u{301}"}]}], "response":{"answer":"🙂 e\u{301}", "citations":[{"id":1,"quote":"🙂 e\u{301}"}], "insufficient_evidence":false}});
    assert!(decode(&value.to_string(), &source).is_ok());
    for quote in ["aa", "é", " ", "不存在"] {
        value["requirements"][0]["evidence"][0]["quote"] = json!(quote);
        assert!(decode(&value.to_string(), &source).is_err());
    }
    for mutate in [
        |v: &mut Value| v["secret"] = json!(true),
        |v: &mut Value| v["requirements"][0]["secret"] = json!(true),
        |v: &mut Value| v["requirements"][0]["evidence"][0]["secret"] = json!(true),
        |v: &mut Value| v["response"]["secret"] = json!(true),
        |v: &mut Value| v["response"]["citations"][0]["secret"] = json!(true),
        |v: &mut Value| v["requirements"] = json!([]),
        |v: &mut Value| v["requirements"] = json!(vec![v["requirements"][0].clone(); 9]),
        |v: &mut Value| v["requirements"][0]["requirement"] = json!("🙂".repeat(161)),
        |v: &mut Value| v["requirements"][0]["requirement"] = json!(" "),
        |v: &mut Value| v["requirements"][0]["evidence"][0]["quote"] = json!("x".repeat(401)),
        |v: &mut Value| v["response"]["answer"] = json!("x".repeat(4001)),
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
        raw.replacen("\"answer\":", "\"answer\":\"x\",\"answer\":", 1),
        raw.clone() + "{}",
        " ".repeat(super::super::MAX_OUTPUT + 1),
    ] {
        assert!(decode(&duplicated, &sources()).is_err());
    }
}
