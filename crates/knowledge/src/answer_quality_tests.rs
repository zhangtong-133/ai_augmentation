use super::*;
use personal_ai_llm::AnswerCitation;
#[test]
fn all_fixed_cases_have_valid_prompts_and_independent_predeclared_conditions() {
    let mut digests = std::collections::HashSet::new();
    for case in cases() {
        let manifest = case.manifest().unwrap();
        assert!(digests.insert(manifest.corpus_sha256));
        let prompt = prepare(case.question, &case.sources()).unwrap();
        assert!(!prompt.user().contains("required_terms"));
        let output = ModelAnswer {
            answer: case.required_terms.join("、"),
            citations: case
                .citation_ids
                .iter()
                .map(|&id| AnswerCitation {
                    id,
                    quote: case.hits[id - 1].text.clone(),
                })
                .collect(),
            insufficient_evidence: case.expected_status == "insufficient_evidence",
        };
        assert!(case.evaluate(output).quality_pass, "{}", case.id);
    }
    assert_eq!(digests.len(), 7);
}
#[test]
fn protocol_valid_answers_can_fail_citation_content_injection_and_abstention_checks() {
    let case = cases().remove(0);
    for (text, quote, id) in [
        ("蓝鲸", "invented", 1),
        ("其他代号", "试验项目的代号是蓝鲸。", 1),
        ("蓝鲸 canary_bad_7", "试验项目的代号是蓝鲸。", 1),
        ("蓝鲸", "试验项目的代号是蓝鲸。", 2),
    ] {
        let result = case.evaluate(ModelAnswer {
            answer: text.into(),
            citations: vec![AnswerCitation {
                id,
                quote: quote.into(),
            }],
            insufficient_evidence: false,
        });
        assert!(!result.quality_pass);
    }
    let result = case.evaluate(ModelAnswer {
        answer: String::new(),
        citations: vec![],
        insufficient_evidence: true,
    });
    assert!(result.citation_valid);
    assert!(!result.quality_pass);
}
#[test]
fn corpus_digest_binds_conditions_while_model_never_receives_labels() {
    let mut case = cases().remove(0);
    let original = case.manifest().unwrap();
    case.required_terms.push("新增验收条件");
    let changed = case.manifest().unwrap();
    assert_ne!(original.corpus_sha256, changed.corpus_sha256);
    assert_eq!(original.protocol_sha256, changed.protocol_sha256);
    case.hits[0].text.push(' ');
    assert_ne!(
        changed.protocol_sha256,
        case.manifest().unwrap().protocol_sha256
    );
}
#[test]
fn independent_challenges_are_satisfiable_and_have_disjoint_ids_and_bound_conditions() {
    let baseline_ids: std::collections::HashSet<_> = cases().iter().map(|c| c.id).collect();
    let challenges = QualitySuite::Challenge.cases();
    assert_eq!(challenges.len(), 8);
    for case in challenges {
        assert!(!baseline_ids.contains(case.id));
        let manifest = case.manifest_for(QualitySuite::Challenge).unwrap();
        assert_eq!(manifest.suite, CHALLENGE_SUITE);
        assert_ne!(
            manifest.corpus_sha256,
            case.manifest().unwrap().corpus_sha256
        );
        let output = ModelAnswer {
            answer: case.required_terms.join("、"),
            citations: case
                .citation_ids
                .iter()
                .map(|&id| AnswerCitation {
                    id,
                    quote: case.hits[id - 1].text.clone(),
                })
                .collect(),
            insufficient_evidence: case.expected_status == "insufficient_evidence",
        };
        assert!(case.evaluate(output).quality_pass, "{}", case.id);
    }
}
