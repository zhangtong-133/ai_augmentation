//! Explicit synthetic-only local answer probes; no database, files or user text input.
use personal_ai_knowledge::answer_quality::{QualityCase, cases};
use personal_ai_llm::{AnswerProvider, LlmError, local::LocalTarget};
use personal_ai_llm_local::answer::{LocalAnswers, preview};
fn target(endpoint: &str, model: &str) -> Result<LocalTarget, &'static str> {
    LocalTarget::new(endpoint, model).map_err(|_| "invalid local answer target")
}
fn manifest(case: &QualityCase, target: &LocalTarget) -> Result<serde_json::Value, &'static str> {
    Ok(
        serde_json::json!({"execution_profile":personal_ai_llm_local::answer::PROFILE,"case":case.manifest().map_err(|_| "invalid corpus")?, "request_sha256":preview(target,case.question,&case.sources()).and_then(|v|v.fingerprint()).map_err(|_| "invalid preview")?}),
    )
}
async fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["--help"] {
        println!(
            "local-answer-benchmark manifest ENDPOINT MODEL\nlocal-answer-benchmark preview CASE ENDPOINT MODEL\nlocal-answer-benchmark case CASE ENDPOINT MODEL --use-local-benchmark\n仅内置合成材料；完整真实评估请用 make local-answer-benchmark，逐次检查显存，不自动重试。"
        );
        return Ok(());
    }
    if args.len() == 3 && args[0] == "manifest" {
        let target = target(&args[1], &args[2])?;
        let all: Vec<_> = cases()
            .iter()
            .map(|case| manifest(case, &target))
            .collect::<Result<_, _>>()?;
        println!(
            "{}",
            serde_json::to_string(&all).map_err(|_| "invalid manifest")?
        );
        return Ok(());
    }
    if !(args.len() == 4 && args[0] == "preview"
        || args.len() == 5 && args[0] == "case" && args[4] == "--use-local-benchmark")
    {
        return Err("invalid local answer benchmark command; see --help");
    }
    let case = cases()
        .into_iter()
        .find(|c| c.id == args[1])
        .ok_or("unknown synthetic case")?;
    let target = target(&args[2], &args[3])?;
    let manifest = manifest(&case, &target)?;
    if args[0] == "preview" {
        println!(
            "{}",
            serde_json::json!({"manifest":manifest,"preview":preview(&target,case.question,&case.sources()).map_err(|_| "invalid preview")?})
        );
        return Ok(());
    }
    let provider = LocalAnswers::new(target.clone()).map_err(|_| "ANSWER_FAILURE=transport")?;
    let started = std::time::Instant::now();
    let output = provider
        .answer(case.question, &case.sources())
        .await
        .map_err(|e| match e {
            LlmError::InvalidResponse(_) => "ANSWER_FAILURE=protocol",
            _ => "ANSWER_FAILURE=transport",
        })?;
    println!(
        "{}",
        serde_json::json!({"manifest":manifest,"endpoint":target.endpoint(),"model":target.model(),"synthetic_only":true,"elapsed_ms":started.elapsed().as_millis(),"protocol_valid":true,"evaluation":case.evaluate(output)})
    );
    Ok(())
}
#[tokio::main]
async fn main() {
    if let Err(error) = run(&std::env::args().skip(1).collect::<Vec<_>>()).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn no_command_can_send_without_explicit_flag_fixed_case_and_loopback_target() {
        for args in [
            vec!["case", "single_fact", "http://127.0.0.1:1", "m"],
            vec![
                "case",
                "unknown",
                "http://127.0.0.1:1",
                "m",
                "--use-local-benchmark",
            ],
            vec![
                "case",
                "single_fact",
                "https://example.com",
                "m",
                "--use-local-benchmark",
            ],
            vec!["preview", "single_fact", "http://127.0.0.1:1", "m", "extra"],
        ] {
            assert!(
                run(&args.into_iter().map(str::to_owned).collect::<Vec<_>>())
                    .await
                    .is_err()
            );
        }
        for case in cases() {
            assert!(
                manifest(
                    &case,
                    &target("http://127.0.0.1:11435", "qwen3:4b-q4_K_M").unwrap()
                )
                .is_ok()
            );
        }
    }
}
