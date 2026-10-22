use crate::{
    PostgresStore,
    answer_authorizations::{self as authorizations, conflict, invalid, key},
    map_error,
    schedules::now,
};
use personal_ai_domain::UserId;
use personal_ai_storage::{
    BoxFuture, StorageError, StorageResult,
    answer_authorizations::{AnswerMaterial, PreparedAnswer},
    answer_executions::{
        AnswerExecution, AnswerExecutionClaim, AnswerExecutionOutcome, AnswerExecutionStore,
        StoredAnswerResult,
    },
};
use sqlx::{PgConnection, Row, postgres::PgRow};
use std::collections::HashSet;
use uuid::Uuid;

const EXECUTION_LIFETIME_MS: i64 = 90_000;

fn metadata(row: &PgRow) -> AnswerExecution {
    AnswerExecution {
        request_id: row.get::<Uuid, _>("request_id").to_string(),
        status: row.get("status"),
        digest: row.get("digest"),
        request_sha256: row.get("request_sha256"),
        started_at_unix_ms: row.get("started_ms"),
        deadline_at_unix_ms: row.get("deadline_ms"),
        sent_at_unix_ms: row.get("sent_ms"),
        finished_at_unix_ms: row.get("finished_ms"),
        preview: None,
        result: None,
    }
}

fn validate_result(result: &StoredAnswerResult, materials: &[AnswerMaterial]) -> StorageResult<()> {
    if serde_json::to_vec(result).map_err(|_| invalid())?.len() > 32_768 {
        return Err(invalid());
    }
    if result.insufficient_evidence {
        return if result.answer.is_empty() && result.citations.is_empty() {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    if result.answer.trim().is_empty()
        || result.answer.chars().count() > 4000
        || result.citations.is_empty()
        || result.citations.len() > materials.len()
    {
        return Err(invalid());
    }
    let mut seen = HashSet::new();
    for citation in &result.citations {
        if citation.id == 0
            || citation.id > materials.len()
            || !seen.insert(citation.id)
            || citation.quote.trim().is_empty()
            || citation.quote.chars().count() > 400
        {
            return Err(invalid());
        }
        let text = &materials[citation.id - 1].text;
        // Match overlapping occurrences at Unicode scalar boundaries, without normalization.
        if text
            .char_indices()
            .filter(|(byte, _)| text[*byte..].starts_with(&citation.quote))
            .take(2)
            .count()
            != 1
        {
            return Err(invalid());
        }
    }
    Ok(())
}

async fn terminal(
    tx: &mut PgConnection,
    owner: Uuid,
    item: &mut AnswerExecution,
    status: &str,
) -> StorageResult<()> {
    let time = now(tx).await?;
    sqlx::query("UPDATE answer_executions SET status=$3,question=NULL,result=NULL,finished_ms=$4 WHERE user_id=$1 AND request_id=$2")
        .bind(owner).bind(key(&item.request_id)?).bind(status).bind(time)
        .execute(tx).await.map_err(map_error)?;
    item.status = status.into();
    item.finished_at_unix_ms = Some(time);
    item.preview = None;
    item.result = None;
    Ok(())
}

async fn expire(tx: &mut PgConnection, owner: Uuid) -> StorageResult<()> {
    sqlx::query("UPDATE answer_executions SET status='unknown',question=NULL,result=NULL,finished_ms=floor(extract(epoch FROM clock_timestamp())*1000)::bigint WHERE user_id=$1 AND status='running' AND deadline_ms<=floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
        .bind(owner).execute(tx).await.map_err(map_error)?;
    Ok(())
}

async fn read(tx: &mut PgConnection, owner: Uuid, request: Uuid) -> StorageResult<AnswerExecution> {
    let row = sqlx::query(
        "SELECT * FROM answer_executions WHERE user_id=$1 AND request_id=$2 FOR UPDATE",
    )
    .bind(owner)
    .bind(request)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_error)?;
    let mut item = metadata(&row);
    if item.status == "running" && item.deadline_at_unix_ms <= now(tx).await? {
        terminal(tx, owner, &mut item, "unknown").await?;
    }
    if !matches!(item.status.as_str(), "running" | "succeeded") {
        return Ok(item);
    }
    let authorization =
        sqlx::query("SELECT * FROM answer_authorizations WHERE user_id=$1 AND request_id=$2")
            .bind(owner)
            .bind(request)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
    let question = row.get::<Option<String>, _>("question");
    let rebuilt = if authorization.get::<String, _>("status") != "consumed"
        || authorization.get::<String, _>("digest") != item.digest
    {
        Err(conflict())
    } else if let Some(question) = question {
        authorizations::rebuild_with_question(tx, owner, &authorization, &question).await
    } else {
        Err(invalid())
    };
    let preview = match rebuilt {
        Ok(preview) if preview.request_sha256 == item.request_sha256 => preview,
        Ok(_)
        | Err(StorageError::NotFound | StorageError::Conflict(_) | StorageError::InvalidData(_)) => {
            terminal(tx, owner, &mut item, "invalidated").await?;
            return Ok(item);
        }
        Err(error) => return Err(error),
    };
    if item.status == "succeeded" {
        let result = row
            .get::<Option<serde_json::Value>, _>("result")
            .and_then(|value| serde_json::from_value::<StoredAnswerResult>(value).ok());
        match result {
            Some(result) if validate_result(&result, &preview.materials).is_ok() => {
                item.result = Some(result);
            }
            _ => {
                terminal(tx, owner, &mut item, "invalidated").await?;
                return Ok(item);
            }
        }
    }
    item.preview = Some(preview);
    if item.status == "running" && item.deadline_at_unix_ms <= now(tx).await? {
        terminal(tx, owner, &mut item, "unknown").await?;
    }
    Ok(item)
}

struct ClaimProof {
    owner: Uuid,
    request: Uuid,
    token: Uuid,
    digest: String,
    preview: PreparedAnswer,
}

impl ClaimProof {
    fn new(claim: &AnswerExecutionClaim) -> StorageResult<Self> {
        Ok(Self {
            owner: key(claim.owner.as_str())?,
            request: key(&claim.request_id)?,
            token: key(&claim.token)?,
            digest: claim.digest.clone(),
            preview: claim.preview.clone(),
        })
    }
}

async fn bound(tx: &mut PgConnection, proof: &ClaimProof) -> StorageResult<AnswerExecution> {
    let row = sqlx::query("SELECT dispatch_token,digest,request_sha256 FROM answer_executions WHERE user_id=$1 AND request_id=$2 FOR UPDATE")
        .bind(proof.owner).bind(proof.request).fetch_one(&mut *tx).await.map_err(map_error)?;
    if row.get::<Uuid, _>("dispatch_token") != proof.token
        || row.get::<String, _>("digest") != proof.digest
        || row.get::<String, _>("request_sha256") != proof.preview.request_sha256
        || proof.preview.request.fingerprint().map_err(|_| invalid())?
            != proof.preview.request_sha256
    {
        return Err(conflict());
    }
    let item = read(tx, proof.owner, proof.request).await?;
    if let Some(preview) = &item.preview
        && serde_json::to_vec(preview).map_err(|_| invalid())?
            != serde_json::to_vec(&proof.preview).map_err(|_| invalid())?
    {
        return Err(conflict());
    }
    Ok(item)
}

impl AnswerExecutionStore for PostgresStore {
    fn claim_answer_execution(
        &self,
        owner: &UserId,
        request: &str,
        digest: &str,
    ) -> BoxFuture<'_, StorageResult<Option<AnswerExecutionClaim>>> {
        let keys = key(owner.as_str()).and_then(|owner| Ok((owner, key(request)?)));
        let digest = digest.to_owned();
        Box::pin(async move {
            let (owner, request) = keys?;
            let mut tx = self.answer_tx(owner).await?;
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM answer_executions WHERE user_id=$1 AND request_id=$2)",
            )
            .bind(owner)
            .bind(request)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
            if exists {
                tx.commit().await.map_err(map_error)?;
                return Ok(None);
            }
            let mut authorization = authorizations::read(&mut tx, owner, request).await?;
            if authorization.status != "authorized" {
                tx.commit().await.map_err(map_error)?;
                return Ok(None);
            }
            if authorization.digest != digest {
                return Err(conflict());
            }
            let preview = authorization.preview.take().ok_or_else(conflict)?;
            let time = now(&mut tx).await?;
            let deadline = (time + EXECUTION_LIFETIME_MS).min(authorization.expires_at_unix_ms);
            let changed = sqlx::query("UPDATE answer_authorizations SET status='consumed',question=NULL WHERE user_id=$1 AND request_id=$2 AND status='authorized' AND expires_ms>floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
                .bind(owner).bind(request).execute(&mut *tx).await.map_err(map_error)?.rows_affected();
            if changed == 0 {
                authorizations::read(&mut tx, owner, request).await?;
                tx.commit().await.map_err(map_error)?;
                return Ok(None);
            }
            let token = Uuid::new_v4();
            sqlx::query("INSERT INTO answer_executions(user_id,request_id,dispatch_token,digest,request_sha256,question,status,started_ms,deadline_ms) VALUES($1,$2,$3,$4,$5,$6,'running',$7,$8)")
                .bind(owner).bind(request).bind(token).bind(&digest).bind(&preview.request_sha256)
                .bind(&preview.question).bind(time).bind(deadline).execute(&mut *tx).await.map_err(map_error)?;
            tx.commit().await.map_err(map_error)?;
            Ok(Some(AnswerExecutionClaim {
                owner: UserId::new(owner.to_string()),
                request_id: request.to_string(),
                token: token.to_string(),
                digest,
                preview,
            }))
        })
    }

    fn begin_answer_execution(
        &self,
        claim: &AnswerExecutionClaim,
    ) -> BoxFuture<'_, StorageResult<bool>> {
        let proof = ClaimProof::new(claim);
        Box::pin(async move {
            let proof = proof?;
            let mut tx = self.answer_tx(proof.owner).await?;
            let item = bound(&mut tx, &proof).await?;
            if item.status != "running" || item.sent_at_unix_ms.is_some() {
                tx.commit().await.map_err(map_error)?;
                return Ok(false);
            }
            let time = now(&mut tx).await?;
            let changed = sqlx::query("UPDATE answer_executions SET sent_ms=$3 WHERE user_id=$1 AND request_id=$2 AND status='running' AND sent_ms IS NULL AND deadline_ms>floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
                .bind(proof.owner).bind(proof.request).bind(time).execute(&mut *tx).await.map_err(map_error)?.rows_affected();
            if changed == 0 {
                read(&mut tx, proof.owner, proof.request).await?;
            }
            tx.commit().await.map_err(map_error)?;
            Ok(changed == 1)
        })
    }

    fn check_answer_execution(
        &self,
        claim: &AnswerExecutionClaim,
    ) -> BoxFuture<'_, StorageResult<bool>> {
        let proof = ClaimProof::new(claim);
        Box::pin(async move {
            let proof = proof?;
            let mut tx = self.answer_tx(proof.owner).await?;
            let item = bound(&mut tx, &proof).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(item.status == "running")
        })
    }

    fn finish_answer_execution(
        &self,
        claim: &AnswerExecutionClaim,
        outcome: AnswerExecutionOutcome,
    ) -> BoxFuture<'_, StorageResult<AnswerExecution>> {
        let proof = ClaimProof::new(claim);
        Box::pin(async move {
            let proof = proof?;
            let mut tx = self.answer_tx(proof.owner).await?;
            let mut item = bound(&mut tx, &proof).await?;
            if item.status != "running" {
                tx.commit().await.map_err(map_error)?;
                return Ok(item);
            }
            let time = now(&mut tx).await?;
            if item.deadline_at_unix_ms <= time {
                terminal(&mut tx, proof.owner, &mut item, "unknown").await?;
            } else {
                match outcome {
                    AnswerExecutionOutcome::Failed => {
                        terminal(&mut tx, proof.owner, &mut item, "failed").await?;
                    }
                    AnswerExecutionOutcome::Unknown => {
                        terminal(&mut tx, proof.owner, &mut item, "unknown").await?;
                    }
                    AnswerExecutionOutcome::Succeeded(result) => {
                        if item.sent_at_unix_ms.is_none() {
                            return Err(conflict());
                        }
                        let preview = item.preview.as_ref().ok_or_else(conflict)?;
                        if validate_result(&result, &preview.materials).is_err() {
                            terminal(&mut tx, proof.owner, &mut item, "failed").await?;
                        } else {
                            let changed = sqlx::query("UPDATE answer_executions SET status='succeeded',result=$3,finished_ms=$4 WHERE user_id=$1 AND request_id=$2 AND status='running' AND deadline_ms>floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
                                .bind(proof.owner).bind(proof.request).bind(serde_json::to_value(result).map_err(|_| invalid())?)
                                .bind(time).execute(&mut *tx).await.map_err(map_error)?.rows_affected();
                            if changed == 0 {
                                terminal(&mut tx, proof.owner, &mut item, "unknown").await?;
                            } else {
                                item = read(&mut tx, proof.owner, proof.request).await?;
                            }
                        }
                    }
                }
            }
            tx.commit().await.map_err(map_error)?;
            Ok(item)
        })
    }

    fn get_answer_execution(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<AnswerExecution>> {
        let keys = key(owner.as_str()).and_then(|owner| Ok((owner, key(request)?)));
        Box::pin(async move {
            let (owner, request) = keys?;
            let mut tx = self.answer_tx(owner).await?;
            let item = read(&mut tx, owner, request).await?;
            tx.commit().await.map_err(map_error)?;
            Ok(item)
        })
    }

    fn list_answer_executions(
        &self,
        owner: &UserId,
    ) -> BoxFuture<'_, StorageResult<Vec<AnswerExecution>>> {
        let owner = key(owner.as_str());
        Box::pin(async move {
            let owner = owner?;
            let mut tx = self.answer_tx(owner).await?;
            expire(&mut tx, owner).await?;
            let rows = sqlx::query("SELECT request_id,status,digest,request_sha256,started_ms,deadline_ms,sent_ms,finished_ms FROM answer_executions WHERE user_id=$1 ORDER BY started_ms DESC,request_id DESC LIMIT 20")
                .bind(owner).fetch_all(&mut *tx).await.map_err(map_error)?;
            let result = rows.iter().map(metadata).collect();
            tx.commit().await.map_err(map_error)?;
            Ok(result)
        })
    }

    fn cancel_answer_execution(
        &self,
        owner: &UserId,
        request: &str,
    ) -> BoxFuture<'_, StorageResult<AnswerExecution>> {
        let keys = key(owner.as_str()).and_then(|owner| Ok((owner, key(request)?)));
        Box::pin(async move {
            let (owner, request) = keys?;
            let mut tx = self.answer_tx(owner).await?;
            let mut item = read(&mut tx, owner, request).await?;
            if matches!(item.status.as_str(), "running" | "succeeded") {
                terminal(&mut tx, owner, &mut item, "cancelled").await?;
            }
            tx.commit().await.map_err(map_error)?;
            Ok(item)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_ai_storage::{
        answer_authorizations::SourceSelection, answer_executions::StoredAnswerCitation,
    };

    fn materials(text: &str) -> Vec<AnswerMaterial> {
        vec![AnswerMaterial {
            selection: SourceSelection {
                document_id: Uuid::new_v4().to_string(),
                ordinal: 0,
            },
            title: "材料".into(),
            source: "test.md".into(),
            text: text.into(),
        }]
    }

    #[test]
    fn stored_results_require_exact_unique_unicode_quotes_and_bounded_shapes() {
        let materials = materials("通行标签为青岚🛰️ e\u{301}。入口为西门。");
        let valid = StoredAnswerResult {
            answer: "标签为青岚🛰️ e\u{301}。".into(),
            citations: vec![StoredAnswerCitation {
                id: 1,
                quote: "青岚🛰️ e\u{301}".into(),
            }],
            insufficient_evidence: false,
        };
        assert!(validate_result(&valid, &materials).is_ok());
        let mut invalid_results = Vec::new();
        for id in [0, 2] {
            let mut result = valid.clone();
            result.citations[0].id = id;
            invalid_results.push(result);
        }
        for quote in ["青岚🛰️ é", "不存在", " "] {
            let mut result = valid.clone();
            result.citations[0].quote = quote.into();
            invalid_results.push(result);
        }
        let mut duplicate = valid.clone();
        duplicate.citations.push(duplicate.citations[0].clone());
        invalid_results.push(duplicate);
        let mut oversized = valid.clone();
        oversized.answer = "字".repeat(4001);
        invalid_results.push(oversized);
        let mut contradiction = valid;
        contradiction.insufficient_evidence = true;
        invalid_results.push(contradiction);
        for result in invalid_results {
            assert!(validate_result(&result, &materials).is_err());
        }
        let absent = StoredAnswerResult {
            answer: String::new(),
            citations: vec![],
            insufficient_evidence: true,
        };
        assert!(validate_result(&absent, &materials).is_ok());
        assert!(
            serde_json::from_value::<StoredAnswerResult>(serde_json::json!({
                "answer":"", "citations":[], "insufficient_evidence":true,"extra":"secret"
            }))
            .is_err()
        );
    }

    #[test]
    fn overlapping_repeated_quotes_are_ambiguous() {
        let result = StoredAnswerResult {
            answer: "重复".into(),
            citations: vec![
                personal_ai_storage::answer_executions::StoredAnswerCitation {
                    id: 1,
                    quote: "哈哈".into(),
                },
            ],
            insufficient_evidence: false,
        };
        assert!(validate_result(&result, &materials("哈哈哈")).is_err());
    }
}
