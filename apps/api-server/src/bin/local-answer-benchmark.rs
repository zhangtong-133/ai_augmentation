//! Explicit synthetic-only local answer probes; no database, files or user text input.
use personal_ai_knowledge::answer_quality::{QualityCase, QualitySuite};
use personal_ai_llm::{AnswerProvider, LlmError, local::LocalTarget};
use personal_ai_llm_local::answer::{LocalAnswers, preview};
use personal_ai_llm_local::ollama_answer::{
    self, AnswerStage, NativeStageGuard, OllamaAnswers, Residency,
};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    LlamaCpp,
    Ollama(Residency),
    Question,
    Guarded,
}
impl Backend {
    fn preview(
        self,
        case: &QualityCase,
        target: &LocalTarget,
    ) -> Result<serde_json::Value, &'static str> {
        match self {
            Self::Guarded => serde_json::to_value(
                ollama_answer::question::guarded::preview(target, case.question, &case.sources())
                    .map_err(|_| "invalid preview")?,
            ),
            Self::Question => serde_json::to_value(
                ollama_answer::question::preview(target, case.question, &case.sources())
                    .map_err(|_| "invalid preview")?,
            ),
            Self::LlamaCpp => serde_json::to_value(
                preview(target, case.question, &case.sources()).map_err(|_| "invalid preview")?,
            ),
            Self::Ollama(residency) => serde_json::to_value(
                ollama_answer::preview_with_residency(
                    target,
                    case.question,
                    &case.sources(),
                    residency,
                )
                .map_err(|_| "invalid preview")?,
            ),
        }
        .map_err(|_| "invalid preview")
    }
    fn provider(self, target: LocalTarget) -> Result<Box<dyn AnswerProvider>, &'static str> {
        match self {
            Self::Guarded => {
                ollama_answer::question::guarded::GuardedAnswers::new(target, self.guard())
                    .map(|v| Box::new(v) as Box<dyn AnswerProvider>)
            }
            Self::Question => ollama_answer::question::QuestionAnswers::new(target, self.guard())
                .map(|v| Box::new(v) as Box<dyn AnswerProvider>),
            Self::LlamaCpp => {
                LocalAnswers::new(target).map(|v| Box::new(v) as Box<dyn AnswerProvider>)
            }
            Self::Ollama(residency) => {
                OllamaAnswers::with_residency(target, self.guard(), residency)
                    .map(|v| Box::new(v) as Box<dyn AnswerProvider>)
            }
        }
        .map_err(|_| "ANSWER_FAILURE=transport")
    }
    fn guard(self) -> Arc<StdioGuard> {
        Arc::new(StdioGuard {
            next: tokio::sync::Mutex::new(0),
            stages: match self {
                Self::Guarded => &[AnswerStage::Plan, AnswerStage::Select, AnswerStage::Verify],
                Self::Question => &[AnswerStage::Plan, AnswerStage::Verify],
                _ => &[AnswerStage::Select, AnswerStage::Verify],
            },
        })
    }
    async fn diagnostic(
        self,
        target: LocalTarget,
        case: &QualityCase,
    ) -> Result<(personal_ai_llm::ModelAnswer, serde_json::Value), LlmError> {
        fn metadata<T: serde::Serialize>(
            (answer, value): (personal_ai_llm::ModelAnswer, T),
        ) -> Result<(personal_ai_llm::ModelAnswer, serde_json::Value), LlmError> {
            serde_json::to_value(value)
                .map(|value| (answer, value))
                .map_err(|_| LlmError::InvalidResponse("invalid diagnostic".into()))
        }
        match self {
            Self::Guarded => {
                ollama_answer::question::guarded::GuardedAnswers::new(target, self.guard())?
                    .answer_with_diagnostics(case.question, &case.sources())
                    .await
                    .and_then(metadata)
            }
            Self::Question => ollama_answer::question::QuestionAnswers::new(target, self.guard())?
                .answer_with_diagnostics(case.question, &case.sources())
                .await
                .and_then(metadata),
            Self::Ollama(residency) => {
                OllamaAnswers::with_residency(target, self.guard(), residency)?
                    .answer_with_diagnostics(case.question, &case.sources())
                    .await
                    .and_then(metadata)
            }
            Self::LlamaCpp => Err(LlmError::InvalidRequest(
                "diagnostic requires Ollama".into(),
            )),
        }
    }
}
// The host runner confirms each native call separately. No acknowledgment means no request.
struct StdioGuard {
    next: tokio::sync::Mutex<u8>,
    stages: &'static [AnswerStage],
}
impl NativeStageGuard for StdioGuard {
    fn permit(
        &self,
        stage: AnswerStage,
    ) -> personal_ai_llm::BoxFuture<'_, personal_ai_llm::LlmResult<()>> {
        Box::pin(async move {
            let mut next = self.next.lock().await;
            if self.stages.get(usize::from(*next)) != Some(&stage) {
                return Err(LlmError::ProviderUnavailable(
                    "native stage permission unavailable".into(),
                ));
            }
            eprintln!("ANSWER_NATIVE_STAGE={}", stage.as_str());
            let mut ack = [0; 6];
            let read = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                tokio::io::stdin().read_exact(&mut ack),
            )
            .await;
            if !matches!(read, Ok(Ok(_))) || &ack != b"allow\n" {
                return Err(LlmError::ProviderUnavailable(
                    "native stage permission unavailable".into(),
                ));
            }
            *next += 1;
            Ok(())
        })
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
        Backend::Guarded => (
            ollama_answer::question::guarded::PROFILE,
            ollama_answer::question::guarded::preview(target, case.question, &case.sources())
                .and_then(|v| v.fingerprint()),
        ),
        Backend::Question => (
            ollama_answer::question::PROFILE,
            ollama_answer::question::preview(target, case.question, &case.sources())
                .and_then(|v| v.fingerprint()),
        ),
        Backend::LlamaCpp => (
            personal_ai_llm_local::answer::PROFILE,
            preview(target, case.question, &case.sources()).and_then(|v| v.fingerprint()),
        ),
        Backend::Ollama(residency) => (
            residency.profile(),
            ollama_answer::preview_with_residency(
                target,
                case.question,
                &case.sources(),
                residency,
            )
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
    let mut residency = None;
    let mut candidate = None;
    while end >= 2 {
        match args[end - 2].as_str() {
            "--candidate" if candidate.is_none() => {
                candidate = Some(match args[end - 1].as_str() {
                    "independent" => "independent",
                    "question" => "question",
                    "guarded" => "guarded",
                    _ => return Err("invalid answer candidate"),
                });
            }
            "--suite" if suite.is_none() => {
                suite = Some(match args[end - 1].as_str() {
                    "baseline" => QualitySuite::Baseline,
                    "challenge" => QualitySuite::Challenge,
                    "coverage" => QualitySuite::Coverage,
                    "extraction" => QualitySuite::Extraction,
                    "decision" => QualitySuite::Decision,
                    "mixed" => QualitySuite::Mixed,
                    "availability" => QualitySuite::Availability,
                    "support" => QualitySuite::Support,
                    _ => return Err("invalid answer suite"),
                });
            }
            "--backend" if backend.is_none() => {
                backend = Some(match args[end - 1].as_str() {
                    "llama.cpp" => Backend::LlamaCpp,
                    "ollama" => Backend::Ollama(Residency::Cold),
                    _ => return Err("invalid answer backend"),
                });
            }
            "--residency" if residency.is_none() => {
                residency = Some(match args[end - 1].as_str() {
                    "cold" => Residency::Cold,
                    "reuse" => Residency::Reuse,
                    _ => return Err("invalid answer residency"),
                });
            }
            _ => break,
        }
        end -= 2;
    }
    let backend = match (backend.unwrap_or(Backend::LlamaCpp), residency) {
        (Backend::Ollama(_), policy) => Backend::Ollama(policy.unwrap_or(Residency::Cold)),
        (Backend::LlamaCpp, None) => Backend::LlamaCpp,
        _ => return Err("residency requires Ollama"),
    };
    let backend = match (backend, candidate) {
        (Backend::Ollama(Residency::Reuse), Some("question")) => Backend::Question,
        (Backend::Ollama(Residency::Reuse), Some("guarded")) => Backend::Guarded,
        (Backend::Ollama(_), Some("independent") | None) | (Backend::LlamaCpp, None) => backend,
        _ => return Err("experimental candidate requires explicit Ollama reuse"),
    };
    Ok((
        &args[..end],
        suite.unwrap_or(QualitySuite::Baseline),
        backend,
    ))
}
async fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["--help"] {
        println!(
            "local-answer-benchmark manifest ENDPOINT MODEL\nlocal-answer-benchmark preview CASE ENDPOINT MODEL\nlocal-answer-benchmark case CASE ENDPOINT MODEL --use-local-benchmark [--guard-native-stages]\nlocal-answer-benchmark diagnose CASE ENDPOINT MODEL --use-local-benchmark --guard-native-stages --backend ollama\n可在末尾添加 --suite baseline|challenge|coverage|extraction|decision|mixed|availability|support --backend llama.cpp|ollama；Ollama 可选 --residency cold|reuse（底层默认 cold，宿主入口默认 reuse）；--candidate independent|question|guarded 默认 independent；question/guarded 必须显式选择 Ollama reuse。question 两步；guarded 三步 plan/select/verify，每步独立授权，首步只读问题。\n仅内置合成材料；diagnose 只增加有界语义计数，不能计入质量门槛；完整真实评估请用 make local-answer-benchmark 或 make ollama-answer-benchmark，逐次检查资源，不自动重试。"
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
    let guarded = backend != Backend::LlamaCpp;
    let diagnostic = args.first().is_some_and(|s| s == "diagnose");
    if !(args.len() == 4 && args[0] == "preview"
        || args.len() == if guarded { 6 } else { 5 }
            && (args[0] == "case" || diagnostic && guarded)
            && args[4] == "--use-local-benchmark"
            && (!guarded || args[5] == "--guard-native-stages"))
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
    let started = std::time::Instant::now();
    let output = if diagnostic {
        backend
            .diagnostic(target.clone(), &case)
            .await
            .map(|(answer, metadata)| (answer, Some(metadata)))
    } else {
        backend
            .provider(target.clone())?
            .answer(case.question, &case.sources())
            .await
            .map(|answer| (answer, None))
    }
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
    let (answer, metadata) = output;
    let mut result = serde_json::json!({"manifest":manifest,"endpoint":target.endpoint(),"model":target.model(),"synthetic_only":true,"elapsed_ms":started.elapsed().as_millis(),"protocol_valid":true,"evaluation":case.evaluate(answer)});
    if let Some(metadata) = metadata {
        result["semantic_diagnostic"] =
            serde_json::to_value(metadata).map_err(|_| "invalid diagnostic")?;
    }
    println!("{result}");
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
    fn v14_independent_requests_preserve_all_v13_conditions() {
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
            ("support", QualitySuite::Support),
        ] {
            let manifests: Vec<_> = suite
                .cases()
                .iter()
                .map(|case| {
                    manifest(case, &target, suite, Backend::Ollama(Residency::Cold)).unwrap()
                })
                .collect();
            actual.insert(name.into(), serde_json::json!(manifests));
        }
        let frozen: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/answer-independent-manifests-ollama-v14.json"
        ))
        .unwrap();
        assert_eq!(serde_json::json!(actual), frozen);
        let previous: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/answer-evidence-first-manifests-ollama-v13.json"
        ))
        .unwrap();
        for (suite, manifests) in &actual {
            assert_eq!(
                manifests.as_array().unwrap().len(),
                previous[suite].as_array().unwrap().len()
            );
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
            vec![
                "case",
                "single_fact",
                "http://127.0.0.1:1",
                "m",
                "--use-local-benchmark",
                "--backend",
                "ollama",
            ],
            vec![
                "case",
                "single_fact",
                "http://127.0.0.1:1",
                "m",
                "--use-local-benchmark",
                "--backend",
                "ollama",
                "--residency",
                "reuse",
            ],
            vec![
                "case",
                "single_fact",
                "http://127.0.0.1:1",
                "m",
                "--use-local-benchmark",
                "--invalid-guard",
                "--backend",
                "ollama",
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
    #[test]
    fn reuse_keeps_every_cold_condition_and_matches_the_new_frozen_request_fingerprints() {
        let target = target("http://127.0.0.1:11434", "qwen3.5:9b").unwrap();
        let frozen: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/answer-residency-manifests-ollama-v15.json"
        ))
        .unwrap();
        let previous: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/answer-independent-manifests-ollama-v14.json"
        ))
        .unwrap();
        assert_eq!(frozen["execution_profile"], Residency::Reuse.profile());
        assert_eq!(frozen["keep_alive_seconds"], 30);
        let mut total = 0;
        for (name, suite) in [
            ("baseline", QualitySuite::Baseline),
            ("challenge", QualitySuite::Challenge),
            ("coverage", QualitySuite::Coverage),
            ("extraction", QualitySuite::Extraction),
            ("decision", QualitySuite::Decision),
            ("mixed", QualitySuite::Mixed),
            ("availability", QualitySuite::Availability),
            ("support", QualitySuite::Support),
        ] {
            let cases = suite.cases();
            assert_eq!(
                cases.len(),
                frozen["suites"][name].as_object().unwrap().len()
            );
            for (index, case) in cases.iter().enumerate() {
                let warm =
                    manifest(case, &target, suite, Backend::Ollama(Residency::Reuse)).unwrap();
                assert_eq!(warm["case"], previous[name][index]["case"]);
                assert_eq!(warm["request_sha256"], frozen["suites"][name][case.id]);
                assert_ne!(
                    warm["request_sha256"],
                    previous[name][index]["request_sha256"]
                );
                total += 1;
            }
        }
        assert_eq!(serde_json::json!(total), frozen["case_count"]);
    }
    #[test]
    fn question_candidate_freezes_all_requests_and_preserves_every_original_condition() {
        let target = target("http://127.0.0.1:11434", "qwen3.5:9b").unwrap();
        let frozen: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/answer-question-manifests-ollama-v16.json"
        ))
        .unwrap();
        let previous: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/answer-independent-manifests-ollama-v14.json"
        ))
        .unwrap();
        assert_eq!(
            frozen["execution_profile"],
            ollama_answer::question::PROFILE
        );
        assert_eq!(
            frozen["native_stages"],
            serde_json::json!(["plan", "verify"])
        );
        assert_eq!(frozen["keep_alive_seconds"], 30);
        let mut total = 0;
        for (name, suite) in [
            ("baseline", QualitySuite::Baseline),
            ("challenge", QualitySuite::Challenge),
            ("coverage", QualitySuite::Coverage),
            ("extraction", QualitySuite::Extraction),
            ("decision", QualitySuite::Decision),
            ("mixed", QualitySuite::Mixed),
            ("availability", QualitySuite::Availability),
            ("support", QualitySuite::Support),
        ] {
            let cases = suite.cases();
            assert_eq!(
                cases.len(),
                frozen["suites"][name].as_object().unwrap().len()
            );
            for (index, case) in cases.iter().enumerate() {
                let actual = manifest(case, &target, suite, Backend::Question).unwrap();
                assert_eq!(actual["case"], previous[name][index]["case"]);
                assert_eq!(actual["request_sha256"], frozen["suites"][name][case.id]);
                assert_ne!(
                    actual["request_sha256"],
                    previous[name][index]["request_sha256"]
                );
                total += 1;
            }
        }
        assert_eq!(serde_json::json!(total), frozen["case_count"]);
    }
    #[test]
    fn guarded_candidate_freezes_three_static_requests_and_preserves_every_original_condition() {
        let target = target("http://127.0.0.1:11434", "qwen3.5:9b").unwrap();
        let frozen: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/answer-guarded-manifests-ollama-v17.json"
        ))
        .unwrap();
        let previous: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/answer-independent-manifests-ollama-v14.json"
        ))
        .unwrap();
        assert_eq!(
            frozen["execution_profile"],
            ollama_answer::question::guarded::PROFILE
        );
        assert_eq!(
            frozen["native_stages"],
            serde_json::json!(["plan", "select", "verify"])
        );
        assert_eq!(frozen["keep_alive_seconds"], 30);
        let mut total = 0;
        for (name, suite) in [
            ("baseline", QualitySuite::Baseline),
            ("challenge", QualitySuite::Challenge),
            ("coverage", QualitySuite::Coverage),
            ("extraction", QualitySuite::Extraction),
            ("decision", QualitySuite::Decision),
            ("mixed", QualitySuite::Mixed),
            ("availability", QualitySuite::Availability),
            ("support", QualitySuite::Support),
        ] {
            let cases = suite.cases();
            assert_eq!(
                cases.len(),
                frozen["suites"][name].as_object().unwrap().len()
            );
            for (index, case) in cases.iter().enumerate() {
                let actual = manifest(case, &target, suite, Backend::Guarded).unwrap();
                assert_eq!(actual["case"], previous[name][index]["case"]);
                assert_eq!(actual["request_sha256"], frozen["suites"][name][case.id]);
                assert_ne!(
                    actual["request_sha256"],
                    previous[name][index]["request_sha256"]
                );
                total += 1;
            }
        }
        assert_eq!(serde_json::json!(total), frozen["case_count"]);
    }
    #[test]
    fn residency_requires_explicit_ollama_backend_and_rejects_invalid_or_repeated_options() {
        for args in [
            vec!["--residency", "reuse"],
            vec!["--backend", "llama.cpp", "--residency", "reuse"],
            vec!["--backend", "ollama", "--residency", "forever"],
            vec![
                "--backend",
                "ollama",
                "--residency",
                "reuse",
                "--residency",
                "cold",
            ],
        ] {
            let args: Vec<_> = args.into_iter().map(str::to_owned).collect();
            assert!(options(&args).is_err() || !options(&args).unwrap().0.is_empty());
        }
        for args in [
            vec!["--backend", "ollama", "--residency", "reuse"],
            vec!["--residency", "reuse", "--backend", "ollama"],
        ] {
            let args: Vec<_> = args.into_iter().map(str::to_owned).collect();
            let (remaining, _, backend) = options(&args).unwrap();
            assert_eq!(remaining, &[] as &[String]);
            assert!(matches!(backend, Backend::Ollama(Residency::Reuse)));
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
    #[tokio::test]
    async fn semantic_diagnostics_require_synthetic_ollama_and_both_native_guards() {
        for suffix in [
            vec![],
            vec!["--backend", "llama.cpp"],
            vec!["--backend", "ollama"],
        ] {
            let args: Vec<_> = [
                vec![
                    "diagnose",
                    "single_fact",
                    "http://127.0.0.1:1",
                    "fixture",
                    "--use-local-benchmark",
                ],
                suffix,
            ]
            .concat()
            .into_iter()
            .map(str::to_owned)
            .collect();
            assert_eq!(
                run(&args).await,
                Err("invalid local answer benchmark command; see --help")
            );
        }
        let args = [
            "diagnose",
            "single_fact",
            "http://127.0.0.1:1",
            "fixture",
            "--guard-native-stages",
            "--backend",
            "ollama",
        ]
        .map(str::to_owned);
        assert!(run(&args).await.is_err());
    }
    #[test]
    fn experimental_candidates_require_explicit_ollama_reuse_and_preserve_default_selection() {
        for candidate in ["question", "guarded"] {
            for suffix in [
                vec!["--candidate", candidate],
                vec!["--backend", "ollama", "--candidate", candidate],
                vec![
                    "--backend",
                    "ollama",
                    "--residency",
                    "cold",
                    "--candidate",
                    candidate,
                ],
                vec!["--candidate", "later"],
                vec![
                    "--backend",
                    "ollama",
                    "--residency",
                    "reuse",
                    "--candidate",
                    candidate,
                    "--candidate",
                    candidate,
                ],
            ] {
                let args: Vec<_> = suffix.into_iter().map(str::to_owned).collect();
                assert!(options(&args).is_err() || !options(&args).unwrap().0.is_empty());
            }
            let args = [
                "--backend",
                "ollama",
                "--residency",
                "reuse",
                "--candidate",
                candidate,
            ]
            .map(str::to_owned);
            assert!(matches!(
                (candidate, options(&args).unwrap().2),
                ("question", Backend::Question) | ("guarded", Backend::Guarded)
            ));
        }
        let args = ["--backend", "ollama", "--residency", "reuse"].map(str::to_owned);
        assert!(matches!(
            options(&args).unwrap().2,
            Backend::Ollama(Residency::Reuse)
        ));
    }
}
