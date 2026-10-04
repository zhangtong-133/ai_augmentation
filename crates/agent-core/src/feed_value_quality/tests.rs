use super::*;
use serde_json::json;
fn output(case: &QualityCase, score: impl Fn(&str) -> Option<u8>, reason: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"items":case.labels.iter().map(|(id,key)|json!({"id":id,"score":score(key),"reason":reason})).collect::<Vec<_>>()})).unwrap()
}
#[test]
fn synthetic_cases_use_real_sharing_and_bound_every_prompt_without_database_fields() {
    let all = cases_for_profile("local-rss-v1").unwrap();
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
fn candidate_preserves_quoted_data_and_criteria_but_changes_exact_request_fingerprint() {
    let old = cases_for_profile("local-rss-v1").unwrap();
    let new = cases_for_profile("local-rss-v2").unwrap();
    for (old, new) in old.iter().zip(&new) {
        let old_request = old.request().unwrap();
        let new_request = new.request().unwrap();
        let old_data: serde_json::Value =
            serde_json::from_str(&old_request.messages[1].content).unwrap();
        let new_data: serde_json::Value =
            serde_json::from_str(new_request.messages[1].content.lines().nth(1).unwrap()).unwrap();
        assert_eq!(old_data, new_data);
        assert!(!new_request.messages[1].content.contains("<system>"));
        assert_eq!(
            old.manifest().unwrap().corpus_sha256,
            new.manifest().unwrap().corpus_sha256
        );
        assert_eq!(
            old.manifest().unwrap().input_digest,
            new.manifest().unwrap().input_digest
        );
        assert_ne!(
            old.manifest().unwrap().request_sha256,
            new.manifest().unwrap().request_sha256
        );
        assert!(new.manifest().unwrap().prompt_bytes <= 5632);
        assert_eq!(new_request.max_output_tokens, Some(2048));
    }
    assert!(cases_for_profile("unknown").is_err());
}
#[test]
fn protocol_valid_output_can_fail_relevance_abstention_and_injection_separately() {
    for case in cases_for_profile("local-rss-v1").unwrap() {
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

#[test]
fn independent_challenge_covers_semantics_injection_and_mixed_abstention() {
    let baseline = cases().unwrap();
    for profile in ["local-rss-v1", "local-rss-v2"] {
        let challenge = cases_for_suite("challenge", profile).unwrap();
        assert_eq!(challenge.len(), 6);
        for case in challenge {
            let manifest = case.manifest().unwrap();
            assert_eq!(manifest.quality_version, CHALLENGE_VERSION);
            assert_ne!(
                manifest.corpus_sha256,
                baseline[0].manifest().unwrap().corpus_sha256
            );
            assert!(manifest.prompt_bytes <= 5632);
            let ideal: Vec<_> = case
                .labels
                .iter()
                .map(|(id, label)| {
                    let empty = case.plan.brief().items[*id - 1]
                        .entry
                        .summary
                        .trim()
                        .is_empty();
                    let relevant = case
                        .checks
                        .iter()
                        .any(|c| matches!(c, Check::Minimum { item, .. } if item == label));
                    let score = if empty {
                        None
                    } else {
                        Some(if relevant { 90 } else { 0 })
                    };
                    let reason = if empty {
                        "摘要为空，无法评分。"
                    } else if relevant {
                        "摘要主题与偏好相关。"
                    } else {
                        "摘要主题与偏好无关。"
                    };
                    json!({"id":id,"score":score,"reason":reason})
                })
                .collect();
            assert!(
                case.evaluate(&serde_json::to_vec(&json!({"items":ideal})).unwrap())
                    .unwrap()
                    .quality_pass
            );
        }
    }
    assert!(cases_for_suite("unknown", "local-rss-v2").is_err());
    assert!(parse(CHALLENGE).is_err());
    let selected = cases_for_suite("baseline", "local-rss-v2").unwrap();
    for (old, new) in baseline.iter().zip(selected) {
        assert_eq!(
            serde_json::to_value(old.manifest().unwrap()).unwrap(),
            serde_json::to_value(new.manifest().unwrap()).unwrap()
        );
    }
}

#[test]
fn classification_preserves_all_data_and_conditions_but_binds_a_new_request() {
    for suite in ["baseline", "challenge"] {
        let old = cases_for_suite(suite, "local-rss-v2").unwrap();
        let new = cases_for_suite(suite, "local-rss-v3").unwrap();
        for (old, new) in old.iter().zip(new) {
            let original: serde_json::Value =
                serde_json::from_str(&old.plan.request().messages[1].content).unwrap();
            let quoted: serde_json::Value =
                serde_json::from_str(&new.request().unwrap().messages[1].content).unwrap();
            assert_eq!(original, quoted);
            let a = old.manifest().unwrap();
            let b = new.manifest().unwrap();
            assert_eq!(a.corpus_sha256, b.corpus_sha256);
            assert_eq!(a.input_digest, b.input_digest);
            assert_ne!(a.request_sha256, b.request_sha256);
            assert!(b.prompt_bytes <= 5632);
            let items: Vec<_> = new.labels.iter().map(|(id, label)| {
                let empty = new.plan.brief().items[*id - 1].entry.summary.trim().is_empty();
                let high = new.checks.iter().any(|c| matches!(c, Check::Minimum {item,..} if item == label));
                json!({"id":id,"category":if empty {"empty"} else if high {"high"} else {"unrelated"}})
            }).collect();
            assert!(
                new.evaluate(&serde_json::to_vec(&json!({"items":items})).unwrap())
                    .unwrap()
                    .quality_pass
            );
        }
    }
}
#[test]
fn classification_rejects_forged_scores_missing_duplicate_unknown_fields_and_ids() {
    let case = cases_for_profile("local-rss-v3").unwrap().remove(0);
    let valid = r#"{"items":[{"id":1,"category":"high"},{"id":2,"category":"partial"},{"id":3,"category":"insufficient"}]}"#;
    let result = case.evaluate(valid.as_bytes()).unwrap();
    assert_eq!(result.scores[&case.labels[&1]], Some(80));
    assert_eq!(result.scores[&case.labels[&2]], Some(40));
    assert_eq!(result.scores[&case.labels[&3]], None);
    for invalid in [
        valid.replace("\"high\"", "\"unknown\""),
        valid.replace("\"high\"", "null"),
        valid.replace("\"high\"", "\"empty\""),
        valid.replace("\"id\":1", "\"id\":0"),
        valid.replace("\"id\":2", "\"id\":1"),
        valid.replace("\"id\":1", "\"id\":1,\"id\":1"),
        valid.replace(
            "\"category\":\"high\"",
            "\"category\":\"high\",\"reason\":\"canary\"",
        ),
        valid.replace(
            "\"category\":\"high\"",
            "\"category\":\"high\",\"score\":100",
        ),
        "{\"items\":[]}".into(),
        " ".repeat(32769),
    ] {
        assert!(case.evaluate(invalid.as_bytes()).is_err());
    }
    let empty = cases_for_profile("local-rss-v3").unwrap().remove(2);
    assert!(
        empty
            .evaluate(
                br#"{"items":[{"id":1,"category":"insufficient"},{"id":2,"category":"empty"}]}"#
            )
            .is_err()
    );
}
