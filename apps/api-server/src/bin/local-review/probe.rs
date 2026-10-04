//! Real-backend compatibility probe with fixed synthetic evidence, never user records.
use personal_ai_domain::UserId;
use personal_ai_learning::model_review::{
    EvidenceFields, ReviewSource, preview, validate_response,
};
use personal_ai_llm::{
    ChatMessage, ChatRequest, Role,
    local::{LocalInference, LocalTarget},
    stream::IgnoreTextDeltas,
};
pub(super) async fn run(target: &LocalTarget) -> Result<(), &'static str> {
    let id = || uuid::Uuid::new_v4().to_string();
    let source = ReviewSource {
        plan_id: id(),
        task_id: id(),
        result_request_id: id(),
        evidence_request_id: id(),
        skill_id: id(),
        skill_revision: 1,
    };
    let preview = preview(
        &UserId::new(id()),
        &source,
        "Rust 所有权",
        "解释所有权，编写独立例子并报告验证和局限。",
        EvidenceFields {
            explanation: "每个值有一个所有者；移动后旧绑定不可再使用。".into(),
            work: "let a = String::from(\"hello\"); let b = a; println!(\"{}\", b);".into(),
            verification:
                "运行例子能输出 hello；添加 println!(\"{}\", a) 编译器报告使用已移动的值。".into(),
            limitations: "未验证借用、生命周期或并发场景，只演示 String 的移动。".into(),
        },
    )
    .map_err(|_| "invalid probe fixture")?;
    let request = ChatRequest {
        messages: vec![
            ChatMessage {
                role: Role::System,
                content: preview.system_prompt().into(),
            },
            ChatMessage {
                role: Role::User,
                content: serde_json::to_string(preview.input())
                    .map_err(|_| "invalid probe fixture")?,
            },
        ],
        temperature: None,
        max_output_tokens: None,
    };
    let started = std::time::Instant::now();
    let raw = personal_ai_llm_local::LocalChatClient::new()
        .map_err(|_| "local client unavailable")?
        .infer(target, &request, &IgnoreTextDeltas)
        .await
        .map_err(|_| "probe stream failed; no automatic retry")?;
    validate_response(&preview, &raw)
        .map_err(|_| "probe advice failed strict digest/citation validation")?;
    println!(
        "{}",
        serde_json::json!({"model":target.model(),"elapsed_ms":started.elapsed().as_millis(),"response_bytes":raw.len(),"strict_protocol_valid":true,"synthetic_evidence_only":true})
    );
    Ok(())
}
