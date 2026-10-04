//! Pure authorization binding. No database, network, model or environment access.
use personal_ai_domain::UserId;
use personal_ai_llm::{AnswerSource, local::LocalTarget, local_answer::preview};
use personal_ai_storage::{
    StorageError, StorageResult,
    answer_authorizations::{AnswerMaterial, AnswerPreparation, PreparedAnswer, SourceBinding},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use uuid::Uuid;
pub const LIFETIME_MS: i64 = 10 * 60 * 1000;
fn invalid() -> StorageError {
    StorageError::InvalidData("invalid answer authorization".into())
}
fn hash(value: &impl Serialize) -> StorageResult<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|_| invalid())?)
    ))
}
fn key(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
/// # Errors
/// Rejects invalid identities, repeated selections, target and question bounds.
pub fn intent_digest(owner: &UserId, input: &AnswerPreparation) -> StorageResult<String> {
    let mut seen = HashSet::new();
    if !key(owner.as_str())
        || !key(&input.request_id)
        || input.question.trim().is_empty()
        || input.question.chars().count() > 1000
        || input.sources.is_empty()
        || input.sources.len() > 5
        || input.sources.iter().any(|s| {
            !key(&s.document_id)
                || s.ordinal > i32::MAX as usize
                || !seen.insert((&s.document_id, s.ordinal))
        })
        || LocalTarget::new(&input.endpoint, &input.model).is_err()
    {
        return Err(invalid());
    }
    hash(&("local-answer-intent-v1", owner.as_str(), input))
}
/// # Errors
/// Rejects mismatched authoritative source order, oversized evidence or invalid lifetimes.
pub fn prepare(
    owner: &UserId,
    input: &AnswerPreparation,
    materials: Vec<AnswerMaterial>,
    created: i64,
    expires: i64,
) -> StorageResult<(PreparedAnswer, Vec<SourceBinding>, String)> {
    let intent = intent_digest(owner, input)?;
    if created < 0
        || created.checked_add(LIFETIME_MS) != Some(expires)
        || materials.len() != input.sources.len()
        || materials
            .iter()
            .zip(&input.sources)
            .any(|(m, s)| m.selection != *s)
    {
        return Err(invalid());
    }
    let sources: Vec<_> = materials
        .iter()
        .enumerate()
        .map(|(i, m)| AnswerSource {
            id: i + 1,
            text: m.text.clone(),
        })
        .collect();
    let request = preview(
        &LocalTarget::new(&input.endpoint, &input.model).map_err(|_| invalid())?,
        &input.question,
        &sources,
    )
    .map_err(|_| invalid())?;
    let request_sha256 = request.fingerprint().map_err(|_| invalid())?;
    let bindings = materials
        .iter()
        .map(|m| {
            Ok(SourceBinding {
                selection: m.selection.clone(),
                content_sha256: hash(m)?,
            })
        })
        .collect::<StorageResult<Vec<_>>>()?;
    let digest = hash(&(
        "local-answer-consent-v1",
        intent,
        &bindings,
        &request_sha256,
        created,
        expires,
    ))?;
    Ok((
        PreparedAnswer {
            question: input.question.clone(),
            materials,
            request,
            request_sha256,
        },
        bindings,
        digest,
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    use personal_ai_storage::answer_authorizations::SourceSelection;
    fn fixture() -> (UserId, AnswerPreparation, Vec<AnswerMaterial>) {
        let selection = SourceSelection {
            document_id: Uuid::new_v4().to_string(),
            ordinal: 0,
        };
        (
            UserId::new(Uuid::new_v4().to_string()),
            AnswerPreparation {
                request_id: Uuid::new_v4().to_string(),
                question: "问题🙂".into(),
                endpoint: "http://127.0.0.1:11435".into(),
                model: "qwen3:4b-q4_K_M".into(),
                sources: vec![selection.clone()],
            },
            vec![AnswerMaterial {
                selection,
                title: "标题".into(),
                source: "local.md".into(),
                text: "原始证据 e\u{301}".into(),
            }],
        )
    }
    #[test]
    fn digest_binds_owner_identity_lifetime_target_question_and_authoritative_metadata() {
        let (owner, input, materials) = fixture();
        let original = prepare(&owner, &input, materials.clone(), 1, 1 + LIFETIME_MS)
            .unwrap()
            .2;
        for mutation in 0..7 {
            let mut i = input.clone();
            let mut m = materials.clone();
            let mut u = owner.clone();
            let mut time = 1;
            match mutation {
                0 => u = UserId::new(Uuid::new_v4().to_string()),
                1 => i.request_id = Uuid::new_v4().to_string(),
                2 => i.question.push(' '),
                3 => i.model = "other".into(),
                4 => m[0].title.push('!'),
                5 => m[0].text.push(' '),
                _ => time = 2,
            }
            assert_ne!(
                original,
                prepare(&u, &i, m, time, time + LIFETIME_MS).unwrap().2
            );
        }
    }
    #[test]
    fn duplicate_forged_source_order_invalid_target_and_local_budget_are_rejected() {
        let (owner, mut input, mut materials) = fixture();
        input.sources.push(input.sources[0].clone());
        assert!(intent_digest(&owner, &input).is_err());
        input.sources.pop();
        materials[0].selection.ordinal = 1;
        assert!(prepare(&owner, &input, materials.clone(), 0, LIFETIME_MS).is_err());
        materials[0].selection.ordinal = 0;
        materials[0].text = "🙂".repeat(1001);
        assert!(prepare(&owner, &input, materials, 0, LIFETIME_MS).is_err());
        input.endpoint = "https://example.com".into();
        assert!(intent_digest(&owner, &input).is_err());
    }
}
