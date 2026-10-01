//! 只接受当前所有者的权威片段，正文与结算在同一事务提交。
use crate::map_error;
use personal_ai_agent_core::model_answer::validate_evidence;
use personal_ai_storage::{
    StorageResult,
    model_execution::{ModelEvidence, RetrievedChunk},
};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

pub(super) async fn collect(
    tx: &mut PgConnection,
    owner: Uuid,
    existing: &[ModelEvidence],
    chunks: &[RetrievedChunk],
    limit: usize,
) -> StorageResult<Option<(Vec<ModelEvidence>, i32)>> {
    if chunks.len() > limit || validate_evidence(existing).is_err() {
        return Ok(None);
    }
    let mut evidence = existing.to_vec();
    let mut output_bytes = 0_i32;
    for chunk in chunks {
        let (Ok(document), Ok(ordinal)) = (
            Uuid::parse_str(&chunk.document_id),
            i32::try_from(chunk.ordinal),
        ) else {
            return Ok(None);
        };
        if ordinal == i32::MAX || chunk.text.len() > 4096 || chunk.text.trim().is_empty() {
            return Ok(None);
        }
        let row = sqlx::query("SELECT title,source,chunks[$3] AS text FROM documents WHERE user_id=$1 AND id=$2 FOR SHARE")
            .bind(owner).bind(document).bind(ordinal + 1).fetch_optional(&mut *tx).await.map_err(map_error)?;
        let Some(row) = row else { return Ok(None) };
        if row.get::<Option<String>, _>("text").as_deref() != Some(chunk.text.as_str()) {
            return Ok(None);
        }
        let item = ModelEvidence {
            id: evidence.len() + 1,
            document_id: document.to_string(),
            ordinal: chunk.ordinal,
            title: row.get("title"),
            source: row.get("source"),
            text: chunk.text.clone(),
        };
        // 统计规范化输出的 JSON 字节数，包含重复命中；至多五条。
        let Ok(bytes) = serde_json::to_vec(&item) else {
            return Ok(None);
        };
        let Ok(bytes) = i32::try_from(bytes.len()) else {
            return Ok(None);
        };
        let Some(total) = output_bytes.checked_add(bytes) else {
            return Ok(None);
        };
        output_bytes = total;
        if !evidence
            .iter()
            .any(|e| e.document_id == item.document_id && e.ordinal == item.ordinal)
        {
            evidence.push(item);
        }
    }
    if output_bytes > 65536 || validate_evidence(&evidence).is_err() {
        return Ok(None);
    }
    Ok(Some((evidence, output_bytes)))
}
