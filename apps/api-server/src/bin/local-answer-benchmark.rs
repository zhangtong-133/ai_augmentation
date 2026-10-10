//! Explicit synthetic-only local answer probes; no database, files or user text input.
use personal_ai_knowledge::answer_quality::{QualityCase, QualitySuite};
use personal_ai_llm::{AnswerProvider, LlmError, local::LocalTarget};
use personal_ai_llm_local::answer::{LocalAnswers, preview};
use personal_ai_llm_local::ollama_answer::{self, OllamaAnswers};
#[derive(Clone, Copy)]
enum Backend {
    LlamaCpp,
    Ollama,
}
impl Backend {
    fn preview(
        self,
        case: &QualityCase,
        target: &LocalTarget,
    ) -> Result<serde_json::Value, &'static str> {
        match self {
            Self::LlamaCpp => serde_json::to_value(
                preview(target, case.question, &case.sources()).map_err(|_| "invalid preview")?,
            ),
            Self::Ollama => serde_json::to_value(
                ollama_answer::preview(target, case.question, &case.sources())
                    .map_err(|_| "invalid preview")?,
            ),
        }
        .map_err(|_| "invalid preview")
    }
    fn provider(self, target: LocalTarget) -> Result<Box<dyn AnswerProvider>, &'static str> {
        match self {
            Self::LlamaCpp => {
                LocalAnswers::new(target).map(|v| Box::new(v) as Box<dyn AnswerProvider>)
            }
            Self::Ollama => {
                OllamaAnswers::new(target).map(|v| Box::new(v) as Box<dyn AnswerProvider>)
            }
        }
        .map_err(|_| "ANSWER_FAILURE=transport")
    }
}
fn target(endpoint: &str, model: &str) -> Result<LocalTarget, &'static str> {
    LocalTarget::new(endpoint, model).map_err(|_| "invalid local answer target")
}
fn manifest(
    case: &QualityCase,
    target: &LocalTarget,
    suite: QualitySuite,
    backend: Backend,
) -> Result<serde_json::Value, &'static str> {
    let (profile, digest) = match backend {
        Backend::LlamaCpp => (
            personal_ai_llm_local::answer::PROFILE,
            preview(target, case.question, &case.sources()).and_then(|v| v.fingerprint()),
        ),
        Backend::Ollama => (
            ollama_answer::PROFILE,
            ollama_answer::preview(target, case.question, &case.sources())
                .and_then(|v| v.fingerprint()),
        ),
    };
    Ok(
        serde_json::json!({"execution_profile":profile,"case":case.manifest_for(suite).map_err(|_| "invalid corpus")?, "request_sha256":digest.map_err(|_| "invalid preview")?}),
    )
}
fn options(args: &[String]) -> Result<(&[String], QualitySuite, Backend), &'static str> {
    let mut end = args.len();
    let mut suite = None;
    let mut backend = None;
    while end >= 2 {
        match args[end - 2].as_str() {
            "--suite" if suite.is_none() => {
                suite = Some(match args[end - 1].as_str() {
                    "baseline" => QualitySuite::Baseline,
                    "challenge" => QualitySuite::Challenge,
                    "coverage" => QualitySuite::Coverage,
                    "extraction" => QualitySuite::Extraction,
                    "decision" => QualitySuite::Decision,
                    "mixed" => QualitySuite::Mixed,
                    "availability" => QualitySuite::Availability,
                    _ => return Err("invalid answer suite"),
                });
            }
            "--backend" if backend.is_none() => {
                backend = Some(match args[end - 1].as_str() {
                    "llama.cpp" => Backend::LlamaCpp,
                    "ollama" => Backend::Ollama,
                    _ => return Err("invalid answer backend"),
                });
            }
            _ => break,
        }
        end -= 2;
    }
    Ok((
        &args[..end],
        suite.unwrap_or(QualitySuite::Baseline),
        backend.unwrap_or(Backend::LlamaCpp),
    ))
}
async fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["--help"] {
        println!(
            "local-answer-benchmark manifest ENDPOINT MODEL\nlocal-answer-benchmark preview CASE ENDPOINT MODEL\nlocal-answer-benchmark case CASE ENDPOINT MODEL --use-local-benchmark\n可在末尾添加 --suite baseline|challenge|coverage|extraction|decision|mixed|availability --backend llama.cpp|ollama\n仅内置合成材料；完整真实评估请用 make local-answer-benchmark 或 make ollama-answer-benchmark，逐次检查资源，不自动重试。"
        );
        return Ok(());
    }
    let (args, suite, backend) = options(args)?;
    if args.len() == 3 && args[0] == "manifest" {
        let target = target(&args[1], &args[2])?;
        let all: Vec<_> = suite
            .cases()
            .iter()
            .map(|case| manifest(case, &target, suite, backend))
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
    let case = suite
        .cases()
        .into_iter()
        .find(|c| c.id == args[1])
        .ok_or("unknown synthetic case")?;
    let target = target(&args[2], &args[3])?;
    let manifest = manifest(&case, &target, suite, backend)?;
    if args[0] == "preview" {
        println!(
            "{}",
            serde_json::json!({"manifest":manifest,"preview":backend.preview(&case, &target)?})
        );
        return Ok(());
    }
    let provider = backend.provider(target.clone())?;
    let started = std::time::Instant::now();
    let output = provider
        .answer(case.question, &case.sources())
        .await
        .map_err(|e| match e {
            LlmError::InvalidResponse(reason) => {
                if let Some(stage) = reason
                    .strip_prefix("ollama answer protocol: ")
                    .filter(|stage| ollama_answer::PROTOCOL_STAGES.contains(stage))
                {
                    eprintln!("ANSWER_PROTOCOL_STAGE={stage}");
                }
                "ANSWER_FAILURE=protocol"
            }
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
    #[test]
    fn original_seven_manifests_and_consent_request_fingerprints_stay_frozen() {
        let target = target("http://127.0.0.1:11435", "qwen3:4b-q4_K_M").unwrap();
        let actual: Vec<_> = QualitySuite::Baseline
            .cases()
            .iter()
            .map(|case| manifest(case, &target, QualitySuite::Baseline, Backend::LlamaCpp).unwrap())
            .collect();
        let frozen: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/answer-baseline-manifests-v1.json"
        ))
        .unwrap();
        assert_eq!(serde_json::json!(actual), frozen);
    }
    #[test]
    fn original_eight_challenge_conditions_and_materials_stay_frozen() {
        let actual: Vec<_> = QualitySuite::Challenge
            .cases()
            .iter()
            .map(|case| case.manifest_for(QualitySuite::Challenge).unwrap())
            .collect();
        let frozen: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/answer-challenge-manifests-v1.json"
        ))
        .unwrap();
        assert_eq!(serde_json::json!(actual), frozen);
    }
    #[test]
    fn new_candidate_keeps_coverage_conditions_frozen_before_the_first_v4_inference() {
        let actual: Vec<_> = QualitySuite::Coverage
            .cases()
            .iter()
            .map(|case| case.manifest_for(QualitySuite::Coverage).unwrap())
            .collect();
        let frozen: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/answer-coverage-manifests-ollama-v4.json"
        ))
        .unwrap();
        let conditions: Vec<_> = frozen
            .as_array()
            .unwrap()
            .iter()
            .map(|item| &item["case"])
            .collect();
        assert_eq!(serde_json::json!(actual), serde_json::json!(conditions));
    }
    #[test]
    fn v10_requests_and_availability_controls_are_frozen_without_relabelling_v9_conditions() {
        let target = target("http://127.0.0.1:11434", "qwen3.5:9b").unwrap();
        let mut actual = serde_json::Map::new();
        for (name, suite) in [
            ("baseline", QualitySuite::Baseline),
            ("challenge", QualitySuite::Challenge),
            ("coverage", QualitySuite::Coverage),
            ("extraction", QualitySuite::Extraction),
            ("decision", QualitySuite::Decision),
            ("mixed", QualitySuite::Mixed),
            ("availability", QualitySuite::Availability),
        ] {
            let manifests: Vec<_> = suite
                .cases()
                .iter()
                .map(|case| manifest(case, &target, suite, Backend::Ollama).unwrap())
                .collect();
            actual.insert(name.into(), serde_json::json!(manifests));
        }
        let frozen: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/answer-availability-manifests-ollama-v10.json"
        ))
        .unwrap();
        assert_eq!(serde_json::json!(actual), frozen);
        let previous: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/answer-mixed-manifests-ollama-v9.json"
        ))
        .unwrap();
        for (suite, manifests) in &actual {
            if suite == "availability" {
                continue;
            }
            for (index, manifest) in manifests.as_array().unwrap().iter().enumerate() {
                assert_eq!(manifest["case"], previous[suite][index]["case"]);
                assert_ne!(
                    manifest["request_sha256"],
                    previous[suite][index]["request_sha256"]
                );
            }
        }
    }
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
        for case in QualitySuite::Baseline.cases() {
            assert!(
                manifest(
                    &case,
                    &target("http://127.0.0.1:11435", "qwen3:4b-q4_K_M").unwrap(),
                    QualitySuite::Baseline,
                    Backend::LlamaCpp
                )
                .is_ok()
            );
        }
    }
    #[tokio::test]
    async fn candidate_selection_stays_offline_and_cannot_relabel_baseline_cases() {
        for args in [
            vec![
                "manifest",
                "http://127.0.0.1:11434",
                "qwen3.5:9b",
                "--suite",
                "extraction",
                "--backend",
                "ollama",
            ],
            vec![
                "manifest",
                "http://127.0.0.1:11434",
                "qwen3.5:9b",
                "--suite",
                "coverage",
                "--backend",
                "ollama",
            ],
            vec![
                "manifest",
                "http://127.0.0.1:11434",
                "qwen3.5:9b",
                "--suite",
                "challenge",
                "--backend",
                "ollama",
            ],
            vec![
                "preview",
                "forged_system",
                "http://127.0.0.1:11434",
                "qwen3.5:9b",
                "--backend",
                "ollama",
                "--suite",
                "challenge",
            ],
        ] {
            assert!(
                run(&args.into_iter().map(str::to_owned).collect::<Vec<_>>())
                    .await
                    .is_ok()
            );
        }
        for options in [
            vec!["--suite", "unknown"],
            vec!["--backend", "cloud"],
            vec!["--suite", "baseline", "--suite", "challenge"],
        ] {
            let args: Vec<String> = [vec!["manifest", "http://127.0.0.1:1", "m"], options]
                .concat()
                .into_iter()
                .map(str::to_owned)
                .collect();
            assert!(run(&args).await.is_err());
        }
        let args = [
            "preview",
            "single_fact",
            "http://127.0.0.1:1",
            "m",
            "--suite",
            "challenge",
        ]
        .map(str::to_owned);
        assert!(run(&args).await.is_err());
    }
}
