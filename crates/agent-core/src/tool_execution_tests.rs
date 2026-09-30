use super::*;
use personal_ai_domain::{ConversationId, UserId};
use personal_ai_storage::{BoxFuture, StorageResult, tool_calls::ToolCallAudit};
use personal_ai_tools::{Tool, ToolError};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Default)]
struct Store {
    record: Mutex<Option<(String, ToolCall)>>,
    finishes: Mutex<Vec<ToolCallFinish>>,
    mode: AtomicUsize,
}
impl ToolCallStore for Store {
    fn start_tool_call(
        &self,
        _: &UserId,
        input: &NewToolCall,
    ) -> BoxFuture<'_, StorageResult<ToolCallStart>> {
        let result = if self.mode.load(Ordering::SeqCst) == 1 {
            Err(StorageError::Unavailable("private database failure".into()))
        } else {
            let mut record = self.record.lock().unwrap();
            match &*record {
                Some((digest, call)) if digest == &input.arguments_digest => {
                    Ok(ToolCallStart::Existing(call.clone()))
                }
                Some(_) => Err(StorageError::Conflict("used".into())),
                None => {
                    let call = ToolCall {
                        request_id: input.request_id.clone(),
                        tool: input.tool.clone(),
                        day: "2026-09-30".into(),
                        status: "running".into(),
                        input_bytes: input.input_bytes,
                        output_bytes: None,
                        created_at_unix_ms: 0,
                        deadline_unix_ms: 60_000,
                        finished_at_unix_ms: None,
                    };
                    *record = Some((input.arguments_digest.clone(), call.clone()));
                    Ok(ToolCallStart::Started(call))
                }
            }
        };
        Box::pin(async { result })
    }
    fn finish_tool_call(
        &self,
        _: &UserId,
        _: &str,
        finish: ToolCallFinish,
    ) -> BoxFuture<'_, StorageResult<ToolCall>> {
        let result = if self.mode.load(Ordering::SeqCst) == 2 {
            Err(StorageError::Unavailable("private database failure".into()))
        } else {
            self.finishes.lock().unwrap().push(finish);
            let mut record = self.record.lock().unwrap();
            let call = &mut record.as_mut().unwrap().1;
            call.status = finish.outcome.as_str().into();
            call.output_bytes = finish.output_bytes;
            call.finished_at_unix_ms = Some(1);
            Ok(call.clone())
        };
        Box::pin(async { result })
    }
    fn get_tool_call(&self, _: &UserId, _: &str) -> BoxFuture<'_, StorageResult<ToolCall>> {
        Box::pin(async { unreachable!() })
    }
    fn audit_tool_calls(
        &self,
        _: &UserId,
        _: Option<&str>,
    ) -> BoxFuture<'_, StorageResult<ToolCallAudit>> {
        Box::pin(async { unreachable!() })
    }
}

struct FixtureTool {
    calls: AtomicUsize,
    mode: usize,
}
impl Tool for FixtureTool {
    fn name(&self) -> &'static str {
        "fixture"
    }
    fn description(&self) -> &'static str {
        "本地工具夹具"
    }
    fn input_schema_json(&self) -> &'static str {
        "{}"
    }
    fn validate(&self, request: &ToolRequest) -> Result<(), ToolError> {
        if request.arguments_json.contains("invalid") {
            Err(ToolError::InvalidArguments("invalid arguments".into()))
        } else {
            Ok(())
        }
    }
    fn execute(
        &self,
        _: &ToolContext,
        _: &ToolRequest,
    ) -> personal_ai_tools::BoxFuture<'_, Result<ToolResponse, ToolError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mode = self.mode;
        Box::pin(async move {
            match mode {
                1 => Err(ToolError::ExecutionFailed("private provider error".into())),
                2 => Ok(ToolResponse {
                    content: "private provider error".into(),
                    is_error: true,
                }),
                3 => Ok(ToolResponse {
                    content: "x".repeat(65537),
                    is_error: false,
                }),
                4 => std::future::pending().await,
                5 => Err(ToolError::PermissionDenied(
                    "private permission error".into(),
                )),
                _ => Ok(ToolResponse {
                    content: "private tool output".into(),
                    is_error: false,
                }),
            }
        })
    }
}
fn context() -> ToolContext {
    ToolContext {
        user_id: UserId::new("owner"),
        conversation_id: ConversationId::new("server-context"),
    }
}
fn runner(mode: usize) -> (AuditedToolExecutor, Arc<Store>, Arc<FixtureTool>) {
    let store = Arc::new(Store::default());
    let tool = Arc::new(FixtureTool {
        calls: AtomicUsize::new(0),
        mode,
    });
    let executor = Arc::new(ToolExecutor::new(vec![tool.clone()]).unwrap());
    (
        AuditedToolExecutor::new(store.clone(), executor),
        store,
        tool,
    )
}
fn request(text: &str) -> ToolRequest {
    ToolRequest {
        arguments_json: text.into(),
    }
}

#[tokio::test]
async fn preflight_and_persistence_failures_prevent_tools_from_running() {
    let (runner, store, tool) = runner(0);
    for (name, input) in [
        ("shell", "{}"),
        ("fixture", "[]"),
        ("fixture", r#"{"invalid":true}"#),
    ] {
        assert!(matches!(
            runner.execute("r", name, &context(), &request(input)).await,
            Err(AuditedToolError::Execution(_))
        ));
        assert!(store.record.lock().unwrap().is_none());
    }
    store.mode.store(1, Ordering::SeqCst);
    assert!(matches!(
        runner
            .execute("r", "fixture", &context(), &request("{}"))
            .await,
        Err(AuditedToolError::Storage(_))
    ));
    assert_eq!(tool.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn successful_calls_record_metadata_and_canonical_replays_never_execute_again() {
    let (runner, store, tool) = runner(0);
    let result = runner
        .execute("r", "fixture", &context(), &request(r#"{"a":1,"b":2}"#))
        .await
        .unwrap();
    assert_eq!(result.call.status, "succeeded");
    assert_eq!(result.call.output_bytes, Some(19));
    assert!(matches!(
        runner
            .execute("r", "fixture", &context(), &request(r#"{ "b":2, "a":1 }"#))
            .await,
        Err(AuditedToolError::AlreadyUsed(_))
    ));
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    assert_eq!(store.finishes.lock().unwrap().len(), 1);
    assert!(
        !serde_json::to_string(&result.call)
            .unwrap()
            .contains("private tool output")
    );
}

#[tokio::test(start_paused = true)]
async fn failure_timeout_and_output_rejection_have_safe_outcomes_without_retries() {
    for (mode, outcome) in [
        (1, ToolCallOutcome::Failed),
        (2, ToolCallOutcome::Failed),
        (3, ToolCallOutcome::OutputRejected),
        (4, ToolCallOutcome::TimedOut),
        (5, ToolCallOutcome::Denied),
    ] {
        let (runner, store, tool) = runner(mode);
        assert!(matches!(
            runner
                .execute("r", "fixture", &context(), &request("{}"))
                .await,
            Err(AuditedToolError::Execution(_))
        ));
        assert_eq!(
            *store.finishes.lock().unwrap(),
            vec![ToolCallFinish {
                outcome,
                output_bytes: None
            }]
        );
        assert!(matches!(
            runner
                .execute("r", "fixture", &context(), &request("{}"))
                .await,
            Err(AuditedToolError::AlreadyUsed(_))
        ));
        assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn losing_final_audit_or_cancelling_future_keeps_the_execution_claim() {
    let (runner, store, tool) = runner(0);
    store.mode.store(2, Ordering::SeqCst);
    assert!(matches!(
        runner
            .execute("r", "fixture", &context(), &request("{}"))
            .await,
        Err(AuditedToolError::Storage(_))
    ));
    store.mode.store(0, Ordering::SeqCst);
    assert!(matches!(
        runner
            .execute("r", "fixture", &context(), &request("{}"))
            .await,
        Err(AuditedToolError::AlreadyUsed(_))
    ));
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    let (runner, store, tool) = self::runner(4);
    let task = tokio::spawn(async move {
        runner
            .execute("r", "fixture", &context(), &request("{}"))
            .await
    });
    tokio::task::yield_now().await;
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.record.lock().unwrap().as_ref().unwrap().1.status,
        "running"
    );
    assert!(store.finishes.lock().unwrap().is_empty());
}
