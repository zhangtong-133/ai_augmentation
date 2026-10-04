use super::*;
use serde_json::json;
fn output(case: &QualityCase, score: impl Fn(&str) -> Option<u8>, reason: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"items":case.labels.iter().map(|(id,key)|json!({"id":id,"score":score(key),"reason":reason})).collect::<Vec<_>>()})).unwrap()
}
#[test]
fn synthetic_cases_use_real_sharing_and_bound_every_prompt_without_database_fields() {
    let all = cases().unwrap();
    assert_eq!(all.len(), 4);
    for case in &all {
        let request = case.request().unwrap();
        let manifest = case.manifest().unwrap();
        assert!(manifest.prompt_bytes <= 5632);
        assert_eq!(request.max_output_tokens, Some(2048));
        assert_eq!(request.messages, case.plan.request().messages);
        for private in [
            OWNER,
            "subscription_id",
            "entry_key",
            "input_digest",
            "checks",
        ] {
            assert!(!request.messages[1].content.contains(private));
        }
        assert_eq!(manifest.corpus_sha256.len(), 64);
        assert_eq!(manifest.request_sha256.len(), 64);
    }
    assert_ne!(
        all[0].manifest().unwrap().input_digest,
        all[1].manifest().unwrap().input_digest
    );
}
#[test]
fn corpus_fingerprint_changes_when_only_quality_criteria_change() {
    let before = cases().unwrap().remove(0).manifest().unwrap();
    let mut corpus: serde_json::Value = serde_json::from_str(CORPUS).unwrap();
    corpus["cases"][0]["checks"][0]["value"] = json!(61);
    let after = parse(&corpus.to_string())
        .unwrap()
        .remove(0)
        .manifest()
        .unwrap();
    assert_ne!(before.corpus_sha256, after.corpus_sha256);
    assert_eq!(before.input_digest, after.input_digest);
    assert_eq!(before.request_sha256, after.request_sha256);
}
#[test]
fn protocol_valid_output_can_fail_relevance_abstention_and_injection_separately() {
    for case in cases().unwrap() {
        let poor = case
            .evaluate(&output(&case, |_| Some(100), "BENCH_INJECTION_CANARY_7D2"))
            .unwrap();
        assert!(poor.protocol_valid);
        assert!(!poor.quality_pass);
        assert!(poor.checks.iter().any(|check| !check.passed));
        let perfect = case
            .evaluate(&output(
                &case,
                |label| match (case.id(), label) {
                    ("rust_preference" | "injected_summary", "rust")
                    | ("python_preference", "python") => Some(90),
                    _ => None,
                },
                "固定合成判断",
            ))
            .unwrap();
        assert!(perfect.quality_pass);
        let json = serde_json::to_string(&perfect).unwrap();
        assert!(!json.contains("固定合成判断"));
    }
}
#[test]
fn malformed_output_is_protocol_failure_without_quality_result() {
    let case = cases().unwrap().remove(0);
    for raw in [
        b"{}".as_slice(),
        b"{\"items\":[]}",
        b"{\"items\":[{\"id\":1,\"score\":100,\"reason\":\"x\"}]}",
    ] {
        assert!(case.evaluate(raw).is_err());
    }
}
#[test]
fn corpus_rejects_unknown_checks_duplicates_and_missing_targets_before_execution() {
    for mutate in 0..4 {
        let mut corpus: serde_json::Value = serde_json::from_str(CORPUS).unwrap();
        match mutate {
            0 => corpus["cases"][0]["checks"][0]["kind"] = json!("unknown"),
            1 => corpus["cases"][1]["id"] = corpus["cases"][0]["id"].clone(),
            2 => corpus["cases"][0]["checks"][0]["item"] = json!("missing"),
            _ => corpus["cases"][0]["checks"][0]["value"] = json!(101),
        }
        assert!(parse(&corpus.to_string()).is_err());
    }
}
