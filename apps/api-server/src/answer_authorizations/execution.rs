use super::{body, finish, map_error, store};
use crate::{AnswerExecutionPermit, ApiError, AppState, auth};
use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use personal_ai_domain::UserId;
use personal_ai_knowledge::{answer::validate_output, retrieval::SearchHit};
use personal_ai_llm::{AnswerSource, LlmError, local_answer::preview_for};
use personal_ai_storage::{
    StorageError, StorageResult,
    answer_authorizations::{AnswerAuthorization, PreparedAnswer},
    answer_executions::{AnswerExecution, AnswerExecutionClaim, AnswerExecutionOutcome},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashMap, time::Duration};

fn available(state: &AppState) -> bool {
    state.answer_execution.is_some() && state.answer_executions.is_some()
}
fn hits(prepared: &PreparedAnswer) -> Vec<SearchHit> {
    prepared
        .materials
        .iter()
        .map(|material| SearchHit {
            document_id: material.selection.document_id.clone(),
            ordinal: material.selection.ordinal,
            title: material.title.clone(),
            source: material.source.clone(),
            text: material.text.clone(),
            score: 1.0,
        })
        .collect()
}
fn execution_view(item: AnswerExecution) -> StorageResult<Value> {
    let result = if let Some(result) = item.result {
        let prepared = item.preview.as_ref().ok_or_else(|| {
            StorageError::Unavailable("answer execution evidence unavailable".into())
        })?;
        Some(
            validate_output(result.into(), &hits(prepared))
                .map_err(|_| StorageError::Unavailable("invalid stored answer execution".into()))?,
        )
    } else {
        None
    };
    Ok(json!({
        "status":item.status,"request_sha256":item.request_sha256,
        "started_at_unix_ms":item.started_at_unix_ms,
        "deadline_at_unix_ms":item.deadline_at_unix_ms,
        "sent_at_unix_ms":item.sent_at_unix_ms,
        "finished_at_unix_ms":item.finished_at_unix_ms,
        "question":item.preview.map(|preview|preview.question),"result":result,
    }))
}
fn grant_view(
    state: &AppState,
    item: AnswerAuthorization,
    execution: Option<AnswerExecution>,
) -> StorageResult<Value> {
    let mut value = serde_json::to_value(item)
        .map_err(|_| StorageError::Unavailable("invalid answer authorization".into()))?;
    value["execution_available"] = json!(available(state));
    value["execution"] = execution
        .map(execution_view)
        .transpose()?
        .unwrap_or(Value::Null);
    Ok(value)
}
pub(super) async fn view(
    state: &AppState,
    owner: &UserId,
    item: AnswerAuthorization,
) -> StorageResult<Value> {
    let execution = if let Some(store) = &state.answer_executions {
        match store.get_answer_execution(owner, &item.request_id).await {
            Ok(execution) => Some(execution),
            Err(StorageError::NotFound) => None,
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    grant_view(state, item, execution)
}
pub(super) async fn list_view(
    state: &AppState,
    owner: &UserId,
    items: Vec<AnswerAuthorization>,
) -> StorageResult<Value> {
    let mut executions: HashMap<_, _> = if let Some(store) = &state.answer_executions {
        store
            .list_answer_executions(owner)
            .await?
            .into_iter()
            .map(|execution| (execution.request_id.clone(), execution))
            .collect()
    } else {
        HashMap::new()
    };
    let items = items
        .into_iter()
        .map(|item| {
            let execution = executions.remove(&item.request_id);
            grant_view(state, item, execution)
        })
        .collect::<StorageResult<Vec<_>>>()?;
    Ok(json!({"items":items,"execution_available":available(state)}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecuteRequest {
    digest: String,
}
fn conflict(code: &'static str) -> ApiError {
    ApiError(StatusCode::CONFLICT, code)
}
pub(super) async fn execute(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    payload: Result<Json<ExecuteRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let owner = auth::current_user(&state, &headers).await?.id;
    auth::mutation_guard(&headers)?;
    let input = body(payload)?;
    let runtime = state.answer_execution.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "answer_execution_disabled",
    ))?;
    let executions = state.answer_executions.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "answer_execution_disabled",
    ))?;
    let item = store(&state)?
        .get_answer_authorization(&owner, &id)
        .await
        .map_err(|error| map_error(&error))?;
    if item.digest != input.digest {
        return Err(conflict("answer_authorization_conflict"));
    }
    if item.status == "consumed" {
        return finish(&state, &headers, &owner, view(&state, &owner, item)).await;
    }
    if item.status != "authorized" {
        return Err(conflict("answer_authorization_conflict"));
    }
    let prepared = item
        .preview
        .ok_or_else(|| conflict("answer_authorization_conflict"))?;
    let sources = sources(&prepared);
    let fingerprint = preview_for(
        runtime.backend(),
        runtime.target(),
        &prepared.question,
        &sources,
    )
    .and_then(|preview| preview.fingerprint())
    .map_err(|_| conflict("answer_execution_target_changed"))?;
    if fingerprint != prepared.request_sha256 {
        return Err(conflict("answer_execution_target_changed"));
    }
    let permit = runtime.admit().await.map_err(|code| {
        ApiError(
            if code == "answer_execution_busy" {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            },
            code,
        )
    })?;
    if auth::current_user(&state, &headers).await?.id != owner {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let claim = executions
        .claim_answer_execution(&owner, &id, &input.digest)
        .await
        .map_err(|error| map_error(&error))?;
    if let Some(claim) = claim {
        let task_state = state.clone();
        let task_headers = headers.clone();
        tokio::spawn(async move { execute_once(task_state, task_headers, claim, permit).await });
    }
    let mut response = finish(&state, &headers, &owner, async {
        let item = store(&state)
            .map_err(|_| StorageError::Unavailable("answer store unavailable".into()))?
            .get_answer_authorization(&owner, &id)
            .await?;
        view(&state, &owner, item).await
    })
    .await?;
    *response.status_mut() = StatusCode::ACCEPTED;
    Ok(response)
}
fn sources(prepared: &PreparedAnswer) -> Vec<AnswerSource> {
    prepared
        .materials
        .iter()
        .enumerate()
        .map(|(index, material)| AnswerSource {
            id: index + 1,
            text: material.text.clone(),
        })
        .collect()
}
async fn still_valid(state: &AppState, headers: &HeaderMap, claim: &AnswerExecutionClaim) -> bool {
    if !auth::current_user(state, headers)
        .await
        .is_ok_and(|user| user.id == claim.owner)
    {
        return false;
    }
    let Some(store) = &state.answer_executions else {
        return false;
    };
    store.check_answer_execution(claim).await.unwrap_or(false)
}
async fn attempt(
    state: &AppState,
    headers: &HeaderMap,
    claim: &AnswerExecutionClaim,
    permit: AnswerExecutionPermit,
) -> AnswerExecutionOutcome {
    let Some(store) = &state.answer_executions else {
        return AnswerExecutionOutcome::Failed;
    };
    let sources = sources(&claim.preview);
    let before_send = Box::pin(async move {
        if !still_valid(state, headers, claim).await
            || !store.begin_answer_execution(claim).await.unwrap_or(false)
            || !still_valid(state, headers, claim).await
        {
            return Err(LlmError::InvalidRequest(
                "answer execution no longer authorized".into(),
            ));
        }
        Ok(())
    });
    let inference = permit.answer_checked(&claim.preview.question, &sources, before_send);
    tokio::pin!(inference);
    let mut poll = tokio::time::interval(Duration::from_millis(500));
    let output = loop {
        tokio::select! {
            output = &mut inference => break output,
            _ = poll.tick() => {
                if !still_valid(state, headers, claim).await {
                    return AnswerExecutionOutcome::Unknown;
                }
            }
        }
    };
    if !still_valid(state, headers, claim).await {
        return AnswerExecutionOutcome::Unknown;
    }
    match output {
        Ok(output) if validate_output(output.clone(), &hits(&claim.preview)).is_ok() => {
            AnswerExecutionOutcome::Succeeded(output.into())
        }
        Ok(_) | Err(LlmError::InvalidResponse(_) | LlmError::InvalidRequest(_)) => {
            AnswerExecutionOutcome::Failed
        }
        Err(_) => AnswerExecutionOutcome::Unknown,
    }
}
async fn execute_once(
    state: AppState,
    headers: HeaderMap,
    claim: AnswerExecutionClaim,
    permit: AnswerExecutionPermit,
) {
    let outcome = tokio::time::timeout(
        Duration::from_secs(55),
        attempt(&state, &headers, &claim, permit),
    )
    .await
    .unwrap_or(AnswerExecutionOutcome::Unknown);
    if let Some(store) = &state.answer_executions {
        // Failure to persist leaves a spent request; its deadline later exposes an unknown result.
        let _ = store.finish_answer_execution(&claim, outcome).await;
    }
}
