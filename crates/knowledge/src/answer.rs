use crate::retrieval::SearchHit;
use personal_ai_llm::{AnswerProvider, AnswerSource, LlmError};
use serde::Serialize;
use std::collections::HashSet;

#[derive(Debug, Serialize)]
pub struct Citation {
    pub id: usize,
    #[serde(flatten)]
    pub hit: SearchHit,
}
#[derive(Debug, Serialize)]
pub struct KnowledgeAnswer {
    pub status: &'static str,
    pub answer: Option<String>,
    pub citations: Vec<Citation>,
}
fn insufficient() -> KnowledgeAnswer {
    KnowledgeAnswer {
        status: "insufficient_evidence",
        answer: None,
        citations: vec![],
    }
}
/// 引用必须来自本次已经复核的检索结果，空结果不调用聊天模型。
///
/// # Errors
/// 模型不可用或输出引用、形状无效时失败。
pub async fn answer(
    provider: &dyn AnswerProvider,
    question: &str,
    hits: &[SearchHit],
) -> Result<KnowledgeAnswer, LlmError> {
    if hits.is_empty() {
        return Ok(insufficient());
    }
    let sources: Vec<_> = hits
        .iter()
        .enumerate()
        .map(|(i, hit)| AnswerSource {
            id: i + 1,
            text: hit.text.clone(),
        })
        .collect();
    let output = provider.answer(question, &sources).await?;
    let invalid = || LlmError::InvalidResponse("invalid answer evidence".into());
    if output.insufficient_evidence {
        if !output.answer.is_empty() || !output.citations.is_empty() {
            return Err(invalid());
        }
        return Ok(insufficient());
    }
    if output.answer.trim().is_empty()
        || output.answer.chars().count() > 4000
        || output.citations.is_empty()
        || output.citations.len() > hits.len()
    {
        return Err(invalid());
    }
    let mut seen = HashSet::new();
    let mut citations = Vec::new();
    for id in output.citations {
        if id == 0 || id > hits.len() || !seen.insert(id) {
            return Err(invalid());
        }
        citations.push(Citation {
            id,
            hit: hits[id - 1].clone(),
        });
    }
    Ok(KnowledgeAnswer {
        status: "answered",
        answer: Some(output.answer),
        citations,
    })
}
