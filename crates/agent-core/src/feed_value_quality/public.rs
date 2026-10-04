//! Frozen, source-derived synthetic fixtures. No network or user documents.
use super::{CaseInput, Check, Corpus, Entry, QualityCase, ValueError, key, parse_version};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
const CALIBRATION: &str = include_str!("public-calibration.json");
const HOLDOUT: &str = include_str!("public-holdout.json");
const CALIBRATION_VERSION: &str = "rss-public-calibration-v1";
const HOLDOUT_VERSION: &str = "rss-public-holdout-v1";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicCorpus {
    version: String,
    material_origin: String,
    annotation_method: String,
    reviewed_on: String,
    sources: Vec<Source>,
    cases: Vec<PublicCase>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    key: String,
    url: String,
    section: String,
    title: String,
    summary: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicCase {
    id: String,
    keywords: Vec<String>,
    entries: Vec<String>,
    checks: Vec<Check>,
    rationale: String,
}
fn decode(input: &str, version: &str) -> Result<PublicCorpus, ValueError> {
    let corpus: PublicCorpus =
        serde_json::from_str(input).map_err(|_| ValueError::InvalidSnapshot)?;
    let date = corpus.reviewed_on.as_bytes();
    if corpus.version != version
        || corpus.material_origin != "public_document_paraphrase"
        || corpus.annotation_method != "agent_prelabelled_unreviewed"
        || date.len() != 10
        || date[4] != b'-'
        || date[7] != b'-'
        || date
            .iter()
            .enumerate()
            .any(|(i, b)| ![4, 7].contains(&i) && !b.is_ascii_digit())
        || corpus.sources.is_empty()
        || corpus.sources.len() > 8
    {
        return Err(ValueError::InvalidSnapshot);
    }
    let mut keys = BTreeSet::new();
    let mut urls = BTreeSet::new();
    for source in &corpus.sources {
        let official = [
            "https://doc.rust-lang.org/",
            "https://docs.python.org/",
            "https://docs.kernel.org/",
        ]
        .iter()
        .any(|prefix| source.url.starts_with(prefix));
        if !key(&source.key)
            || !keys.insert(&source.key)
            || !urls.insert(&source.url)
            || !official
            || source.url.strip_suffix(".html").is_none()
            || source.url.contains("..")
            || !source
                .url
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/:._-".contains(&b))
            || source.section.trim().is_empty()
            || source.section.len() > 512
            || source.summary.trim().is_empty()
        {
            return Err(ValueError::InvalidSnapshot);
        }
    }
    Ok(corpus)
}
fn build(
    input: &str,
    version: &'static str,
    corpus: PublicCorpus,
) -> Result<Vec<QualityCase>, ValueError> {
    let sources: BTreeMap<_, _> = corpus
        .sources
        .into_iter()
        .map(|s| (s.key.clone(), s))
        .collect();
    let mut used = BTreeSet::new();
    let mut cases = Vec::new();
    for case in corpus.cases {
        if case.rationale.trim().is_empty() || case.rationale.len() > 2048 {
            return Err(ValueError::InvalidSnapshot);
        }
        let mut entries = Vec::new();
        for key in case.entries {
            let source = sources.get(&key).ok_or(ValueError::InvalidSnapshot)?;
            used.insert(key.clone());
            entries.push(Entry {
                key,
                title: source.title.clone(),
                summary: source.summary.clone(),
            });
        }
        cases.push(CaseInput {
            id: case.id,
            keywords: case.keywords,
            entries,
            checks: case.checks,
        });
    }
    if used.len() != sources.len() {
        return Err(ValueError::InvalidSnapshot);
    }
    let generated = serde_json::to_string(&Corpus {
        version: version.into(),
        cases,
    })
    .map_err(|_| ValueError::InvalidSnapshot)?;
    let mut cases = parse_version(&generated, version)?;
    // Bind source URLs, reviewed date and pre-label rationale, not only model input.
    for case in &mut cases {
        case.corpus_sha256 = format!("{:x}", Sha256::digest(input));
    }
    Ok(cases)
}
pub(super) fn cases(suite: &str) -> Result<Vec<QualityCase>, ValueError> {
    let calibration = decode(CALIBRATION, CALIBRATION_VERSION)?;
    let holdout = decode(HOLDOUT, HOLDOUT_VERSION)?;
    validate_split(&calibration, &holdout)?;
    match suite {
        "public_calibration" => build(CALIBRATION, CALIBRATION_VERSION, calibration),
        "public_holdout" => build(HOLDOUT, HOLDOUT_VERSION, holdout),
        _ => Err(ValueError::InvalidSnapshot),
    }
}
fn validate_split(calibration: &PublicCorpus, holdout: &PublicCorpus) -> Result<(), ValueError> {
    if calibration
        .sources
        .iter()
        .any(|a| holdout.sources.iter().any(|b| a.url == b.url))
    {
        return Err(ValueError::InvalidSnapshot);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
