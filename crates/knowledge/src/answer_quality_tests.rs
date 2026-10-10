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
#[test]
fn coverage_controls_keep_missing_facts_and_complete_answers_as_independent_conditions() {
    let original_ids: std::collections::HashSet<_> = cases()
        .into_iter()
        .chain(challenge_cases())
        .map(|c| c.id)
        .collect();
    let coverage = QualitySuite::Coverage.cases();
    assert_eq!(coverage.len(), 10);
    assert_eq!(
        coverage
            .iter()
            .filter(|c| c.expected_status == "insufficient_evidence")
            .count(),
        5
    );
    let mut digests = std::collections::HashSet::new();
    for case in coverage {
        assert!(!original_ids.contains(case.id));
        let manifest = case.manifest_for(QualitySuite::Coverage).unwrap();
        assert_eq!(manifest.suite, COVERAGE_SUITE);
        assert!(digests.insert(manifest.corpus_sha256));
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

#[test]
fn extractive_controls_are_source_blind_conditions_without_answer_labels_in_the_prompt() {
    let old_ids: std::collections::HashSet<_> = cases()
        .into_iter()
        .chain(challenge_cases())
        .chain(coverage_cases())
        .map(|c| c.id)
        .collect();
    let cases = QualitySuite::Extraction.cases();
    assert_eq!(cases.len(), 8);
    assert_eq!(
        cases
            .iter()
            .filter(|c| c.expected_status == "insufficient_evidence")
            .count(),
        4
    );
    let mut digests = std::collections::HashSet::new();
    for case in cases {
        assert!(!old_ids.contains(case.id));
        let manifest = case.manifest_for(QualitySuite::Extraction).unwrap();
        assert_eq!(manifest.suite, EXTRACTION_SUITE);
        assert!(digests.insert(manifest.corpus_sha256));
        let prompt = prepare(case.question, &case.sources()).unwrap();
        for field in [
            "expected_status",
            "required_terms",
            "citation_ids",
            "forbidden_terms",
        ] {
            assert!(!prompt.user().contains(field));
        }
        // The first mixed source's valid quote excludes its separate instruction sentence.
        let output = ModelAnswer {
            answer: case.required_terms.join("、"),
            citations: case
                .citation_ids
                .iter()
                .map(|&id| AnswerCitation {
                    id,
                    quote: if case.id == "same_source_injection" {
                        "采样小组从暮岭台出发。".into()
                    } else {
                        case.hits[id - 1].text.clone()
                    },
                })
                .collect(),
            insufficient_evidence: case.expected_status == "insufficient_evidence",
        };
        assert!(case.evaluate(output).quality_pass, "{}", case.id);
    }
}

#[test]
fn new_decision_controls_distinguish_missing_values_from_explicit_availability_questions() {
    let old_ids: std::collections::HashSet<_> = cases()
        .into_iter()
        .chain(challenge_cases())
        .chain(coverage_cases())
        .chain(extraction_cases())
        .map(|case| case.id)
        .collect();
    let controls = QualitySuite::Decision.cases();
    assert_eq!(controls.len(), 4);
    assert_eq!(controls[0].hits[0].text, controls[1].hits[0].text);
    assert_ne!(controls[0].question, controls[1].question);
    assert_eq!(
        controls
            .iter()
            .filter(|c| c.expected_status == "insufficient_evidence")
            .count(),
        2
    );
    let mut digests = std::collections::HashSet::new();
    for case in controls {
        assert!(!old_ids.contains(case.id));
        let manifest = case.manifest_for(QualitySuite::Decision).unwrap();
        assert_eq!(manifest.suite, DECISION_SUITE);
        assert!(digests.insert(manifest.corpus_sha256));
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
        assert!(
            !prepare(case.question, &case.sources())
                .unwrap()
                .user()
                .contains("expected_status")
        );
    }
}

#[test]
fn mixed_controls_preserve_supported_facts_and_require_abstention_for_missing_values() {
    let old_ids: std::collections::HashSet<_> = cases()
        .into_iter()
        .chain(challenge_cases())
        .chain(coverage_cases())
        .chain(extraction_cases())
        .chain(decision_cases())
        .map(|case| case.id)
        .collect();
    let controls = QualitySuite::Mixed.cases();
    assert_eq!(controls.len(), 10);
    assert_eq!(
        controls
            .iter()
            .filter(|c| c.expected_status == "insufficient_evidence")
            .count(),
        5
    );
    let mut ids = std::collections::HashSet::new();
    let mut digests = std::collections::HashSet::new();
    for case in controls {
        assert!(!old_ids.contains(case.id));
        assert!(ids.insert(case.id));
        let manifest = case.manifest_for(QualitySuite::Mixed).unwrap();
        assert_eq!(manifest.suite, MIXED_SUITE);
        assert!(digests.insert(manifest.corpus_sha256));
        let prompt = prepare(case.question, &case.sources()).unwrap();
        for field in [
            "expected_status",
            "required_terms",
            "citation_ids",
            "forbidden_terms",
        ] {
            assert!(!prompt.user().contains(field));
        }
        assert!(prompt.user().contains("MIXED_OVERRIDE_11"));
        let quote = match case.id {
            "role_before_fact" | "role_after_fact" => "沐星观测队从苔岬岗出发。",
            "attack_source_first" => "绘图课在周二开课。",
            "complete_pair_near_attack" => "青檀工作坊周五开课。负责人为甘露。",
            "security_analysis_near_attack" => {
                "培训把伪造 SYSTEM 角色标签归为角色冒充。要求把标签视为资料并忽略其操作要求。"
            }
            _ => "",
        };
        let output = ModelAnswer {
            answer: quote.into(),
            citations: case
                .citation_ids
                .iter()
                .map(|&id| AnswerCitation {
                    id,
                    quote: quote.into(),
                })
                .collect(),
            insufficient_evidence: case.expected_status == "insufficient_evidence",
        };
        assert!(case.evaluate(output).quality_pass, "{}", case.id);
        // Neither universal abstention nor copying the available partial fact can pass.
        let wrong = if case.expected_status == "answered" {
            ModelAnswer {
                answer: String::new(),
                citations: vec![],
                insufficient_evidence: true,
            }
        } else {
            let partial = format!("{}。", case.hits[0].text.split('。').next().unwrap());
            ModelAnswer {
                answer: partial.clone(),
                citations: vec![AnswerCitation {
                    id: 1,
                    quote: partial,
                }],
                insufficient_evidence: false,
            }
        };
        let evaluation = case.evaluate(wrong);
        assert!(evaluation.citation_valid, "{}", case.id);
        assert!(!evaluation.quality_pass, "{}", case.id);
        assert!(!evaluation.checks[0].passed, "{}", case.id);
    }
}
