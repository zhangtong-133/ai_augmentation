use super::*;
use personal_ai_llm::AnswerCitation;

fn hit(text: &str) -> SearchHit {
    SearchHit {
        document_id: "server-document".into(),
        title: "server-title".into(),
        source: "server-source".into(),
        ordinal: 2,
        text: text.into(),
        score: 0.9,
    }
}
fn output(id: usize, quote: &str) -> ModelAnswer {
    ModelAnswer {
        answer: "supported answer".into(),
        citations: vec![AnswerCitation {
            id,
            quote: quote.into(),
        }],
        insufficient_evidence: false,
    }
}
#[test]
fn citations_use_server_metadata_and_unicode_scalar_ranges() {
    let text = "前🙂言：证据 e\u{301}\n结尾";
    let result = validate_output(output(2, "证据 e\u{301}\n"), &[hit("other"), hit(text)]).unwrap();
    let citation = &result.citations[0];
    assert_eq!(citation.id, 2);
    assert_eq!(citation.hit.document_id, "server-document");
    assert_eq!(citation.hit.title, "server-title");
    assert_eq!(citation.hit.source, "server-source");
    assert_eq!(citation.hit.ordinal, 2);
    assert_eq!((citation.quote_start, citation.quote_end), (4, 10));
    assert_eq!(
        text.chars()
            .skip(citation.quote_start)
            .take(citation.quote_end - citation.quote_start)
            .collect::<String>(),
        citation.quote
    );
}
#[test]
fn invented_normalized_empty_oversized_and_ambiguous_quotes_are_rejected() {
    for (text, quote) in [
        ("evidence", "invented"),
        ("evidence", "Evidence"),
        (" evidence ", ""),
        (" \n ", " \n "),
        ("one  two", "one two"),
        ("e\u{301}", "é"),
        ("重复 重复", "重复"),
        ("aaaa", "aaa"),
        ("🙂🙂🙂", "🙂🙂"),
    ] {
        assert!(
            validate_output(output(1, quote), &[hit(text)]).is_err(),
            "{text:?} / {quote:?}"
        );
    }
    let boundary = "🙂".repeat(400);
    assert!(validate_output(output(1, &boundary), &[hit(&boundary)]).is_ok());
    let too_long = "🙂".repeat(401);
    assert!(validate_output(output(1, &too_long), &[hit(&too_long)]).is_err());
}
#[test]
fn every_citation_must_be_valid_and_duplicate_or_unknown_ids_are_rejected() {
    for id in [0, 2, usize::MAX] {
        assert!(validate_output(output(id, "evidence"), &[hit("evidence")]).is_err());
    }
    let mut answer = output(1, "evidence");
    answer.citations.push(answer.citations[0].clone());
    assert!(validate_output(answer, &[hit("evidence"), hit("other")]).is_err());
    let mut answer = output(1, "evidence");
    answer.citations.push(AnswerCitation {
        id: 2,
        quote: "invented".into(),
    });
    assert!(validate_output(answer, &[hit("evidence"), hit("other")]).is_err());
    let mut answer = output(1, "evidence");
    answer.insufficient_evidence = true;
    assert!(validate_output(answer, &[hit("evidence")]).is_err());
}
