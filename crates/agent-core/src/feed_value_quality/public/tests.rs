use super::super::cases_for_suite;
use super::*;
use serde_json::json;
#[test]
fn source_metadata_is_frozen_but_never_shared_as_model_instructions() {
    for suite in ["public_calibration", "public_holdout"] {
        let all = cases_for_suite(suite, "local-rss-v4").unwrap();
        assert_eq!(all.len(), 4);
        for case in all {
            let manifest = case.manifest().unwrap();
            assert_eq!(manifest.material_origin, Some("public_document_paraphrase"));
            assert!(manifest.prompt_bytes <= 5632);
            assert_eq!(manifest.items.len(), 3);
            let request = case.request().unwrap();
            for hidden in [
                "https://",
                "agent_prelabelled",
                "rationale",
                "reviewed_on",
                "checks",
            ] {
                assert!(request.messages.iter().all(|m| !m.content.contains(hidden)));
            }
            let labels: BTreeSet<_> = case.labels.values().collect();
            assert_eq!(labels.len(), 3);
        }
    }
}
#[test]
fn source_or_annotation_edits_change_corpus_binding_without_changing_model_request() {
    let before = cases("public_calibration")
        .unwrap()
        .remove(0)
        .manifest()
        .unwrap();
    for field in ["reviewed_on", "rationale", "url"] {
        let mut data: serde_json::Value = serde_json::from_str(CALIBRATION).unwrap();
        match field {
            "reviewed_on" => data[field] = json!("2026-10-05"),
            "rationale" => data["cases"][0][field] = json!("不同预标注理由"),
            _ => data["sources"][0][field] = json!("https://doc.rust-lang.org/book/other.html"),
        }
        let input = data.to_string();
        let after = build(
            &input,
            CALIBRATION_VERSION,
            decode(&input, CALIBRATION_VERSION).unwrap(),
        )
        .unwrap()
        .remove(0)
        .manifest()
        .unwrap();
        assert_ne!(before.corpus_sha256, after.corpus_sha256);
        assert_eq!(before.request_sha256, after.request_sha256);
    }
}
#[test]
fn malformed_provenance_missing_sources_and_false_human_review_are_rejected() {
    for mutation in 0..5 {
        let mut data: serde_json::Value = serde_json::from_str(CALIBRATION).unwrap();
        match mutation {
            0 => data["sources"][0]["url"] = json!("https://docs.python.org.evil.invalid/a.html"),
            1 => data["cases"][0]["entries"][0] = json!("missing"),
            2 => data["annotation_method"] = json!("human_verified"),
            3 => data["cases"][0]["rationale"] = json!(""),
            _ => data["sources"][1]["url"] = data["sources"][0]["url"].clone(),
        }
        let input = data.to_string();
        assert!(
            decode(&input, CALIBRATION_VERSION)
                .and_then(|c| build(&input, CALIBRATION_VERSION, c))
                .is_err()
        );
    }
}
#[test]
fn partial_requires_a_numeric_middle_score_and_unrelated_cannot_abstain() {
    let case = cases_for_suite("public_calibration", "local-rss-v4")
        .unwrap()
        .pop()
        .unwrap();
    for (linux, expected) in [
        ("partial", true),
        ("high", false),
        ("unrelated", false),
        ("insufficient", false),
    ] {
        let raw = json!({"items":case.labels.iter().map(|(id,key)|json!({"id":id,"category":match key.as_str(){"rust"=>"high","linux"=>linux,_=>"unrelated"}})).collect::<Vec<_>>()});
        assert_eq!(
            case.evaluate(&serde_json::to_vec(&raw).unwrap())
                .unwrap()
                .quality_pass,
            expected
        );
    }
}

#[test]
fn holdout_cannot_reuse_a_calibration_document_even_under_a_different_label() {
    let calibration = decode(CALIBRATION, CALIBRATION_VERSION).unwrap();
    let mut holdout = decode(HOLDOUT, HOLDOUT_VERSION).unwrap();
    validate_split(&calibration, &holdout).unwrap();
    holdout.sources[0].key = "renamed".into();
    holdout.sources[0]
        .url
        .clone_from(&calibration.sources[0].url);
    assert!(validate_split(&calibration, &holdout).is_err());
}
