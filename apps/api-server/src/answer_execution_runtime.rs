//! Explicit local answer execution, independent of paid providers and quality experiments.
use personal_ai_llm::{
    AnswerProvider, AnswerSource, BoxFuture, LlmError, LlmResult, ModelAnswer, local::LocalTarget,
};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const GUARD_UNAVAILABLE: &str = "answer_execution_resource_unavailable";

/// A trusted resource check receives no question, source text, session or grant.
pub trait AnswerExecutionGuard: Send + Sync {
    fn check(&self) -> BoxFuture<'_, Result<(), &'static str>>;
}

pub struct AnswerExecutionRuntime {
    target: LocalTarget,
    provider: Arc<dyn AnswerProvider>,
    guard: Arc<dyn AnswerExecutionGuard>,
    slots: Arc<Semaphore>,
}

impl AnswerExecutionRuntime {
    /// Builds an execution runtime from trusted adapters; does not run a model.
    /// The provider must use the same target and versioned request as the consent preview.
    #[must_use]
    pub fn new(
        target: LocalTarget,
        provider: Arc<dyn AnswerProvider>,
        guard: Arc<dyn AnswerExecutionGuard>,
    ) -> Self {
        Self {
            target,
            provider,
            guard,
            slots: Arc::new(Semaphore::new(1)),
        }
    }

    #[must_use]
    pub fn target(&self) -> &LocalTarget {
        &self.target
    }

    /// Reserves the only execution slot and checks resources before a grant is consumed.
    /// # Errors
    /// Rejects concurrent execution or unavailable resource admission without any model send.
    pub async fn admit(self: &Arc<Self>) -> Result<AnswerExecutionPermit, &'static str> {
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| "answer_execution_busy")?;
        self.guard.check().await.map_err(|_| GUARD_UNAVAILABLE)?;
        Ok(AnswerExecutionPermit {
            runtime: self.clone(),
            slot,
        })
    }
}

/// A one-use, process-local permit. Storage remains responsible for claiming the grant once.
pub struct AnswerExecutionPermit {
    runtime: Arc<AnswerExecutionRuntime>,
    slot: OwnedSemaphorePermit,
}

impl AnswerExecutionPermit {
    /// Rechecks resources immediately before the single provider call.
    /// # Errors
    /// Rejects unavailable resources or propagates the local provider's bounded error.
    #[must_use = "Await the future to send at most one local answer request."]
    pub fn answer<'a>(
        self,
        question: &'a str,
        sources: &'a [AnswerSource],
    ) -> BoxFuture<'a, LlmResult<ModelAnswer>> {
        self.answer_checked(question, sources, Box::pin(async { Ok(()) }))
    }

    /// Rechecks resources, then revalidates the caller's live permission before the send.
    /// # Errors
    /// Rejects unavailable resources, revoked permission or a bounded local provider error.
    #[must_use = "Await the future to validate permission and send at most one local request."]
    pub fn answer_checked<'a>(
        self,
        question: &'a str,
        sources: &'a [AnswerSource],
        before_send: BoxFuture<'a, LlmResult<()>>,
    ) -> BoxFuture<'a, LlmResult<ModelAnswer>> {
        Box::pin(async move {
            // Keep the slot through the provider call, including cancellation and errors.
            let _slot = self.slot;
            self.runtime.guard.check().await.map_err(|_| {
                LlmError::ProviderUnavailable("local answer resource admission unavailable".into())
            })?;
            before_send.await?;
            self.runtime.provider.answer(question, sources).await
        })
    }
}

struct CommandGuard {
    script: PathBuf,
    node_binary: String,
    target: LocalTarget,
    timeout: Duration,
}

impl AnswerExecutionGuard for CommandGuard {
    fn check(&self) -> BoxFuture<'_, Result<(), &'static str>> {
        Box::pin(async move {
            let mut command = tokio::process::Command::new(&self.node_binary);
            command
                .arg(&self.script)
                .arg("answer-admit")
                .arg(self.target.endpoint())
                .arg(self.target.model())
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            for key in ["PATH", "HOME"] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
            let status = tokio::time::timeout(self.timeout, command.status())
                .await
                .map_err(|_| GUARD_UNAVAILABLE)?
                .map_err(|_| GUARD_UNAVAILABLE)?;
            if status.success() {
                Ok(())
            } else {
                Err(GUARD_UNAVAILABLE)
            }
        })
    }
}

struct Settings {
    target: LocalTarget,
    script: PathBuf,
    node_binary: String,
}

fn settings(env: impl Fn(&str) -> Option<String>) -> Result<Option<Settings>, &'static str> {
    match env("LOCAL_ANSWER_EXECUTION_ENABLED").as_deref() {
        None | Some("false") => return Ok(None),
        Some("true") => {}
        _ => return Err("LOCAL_ANSWER_EXECUTION_ENABLED must be true or false"),
    }
    let endpoint = env("LOCAL_ANSWER_ENDPOINT").ok_or("LOCAL_ANSWER_ENDPOINT is required")?;
    let model = env("LOCAL_ANSWER_MODEL").ok_or("LOCAL_ANSWER_MODEL is required")?;
    let target = LocalTarget::new(&endpoint, &model).map_err(|_| "invalid local answer target")?;
    let script = env("LOCAL_ANSWER_GUARD_SCRIPT")
        .filter(|value| !value.contains('\0'))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("LOCAL_ANSWER_GUARD_SCRIPT must be an absolute script path")?;
    let node_binary = env("LOCAL_ANSWER_NODE_BINARY").unwrap_or_else(|| "node".into());
    if node_binary != "node"
        && (!std::path::Path::new(&node_binary).is_absolute() || node_binary.contains('\0'))
    {
        return Err("LOCAL_ANSWER_NODE_BINARY must be node or an absolute executable path");
    }
    Ok(Some(Settings {
        target,
        script,
        node_binary,
    }))
}

/// Loads a separate local execution runtime; missing or false enablement performs no I/O.
/// # Errors
/// Rejects invalid fixed target/guard configuration or local HTTP client initialization.
pub fn answer_execution_from_env()
-> Result<Option<Arc<AnswerExecutionRuntime>>, Box<dyn std::error::Error>> {
    let Some(settings) = settings(|key| std::env::var(key).ok())? else {
        return Ok(None);
    };
    let provider = Arc::new(personal_ai_llm_local::answer::LocalAnswers::new(
        settings.target.clone(),
    )?);
    let guard = Arc::new(CommandGuard {
        script: settings.script,
        node_binary: settings.node_binary,
        target: settings.target.clone(),
        timeout: Duration::from_secs(3),
    });
    Ok(Some(Arc::new(AnswerExecutionRuntime::new(
        settings.target,
        provider,
        guard,
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn target() -> LocalTarget {
        LocalTarget::new("http://127.0.0.1:11435", "qwen3:4b-fixture").unwrap()
    }
    fn configuration(key: &str) -> Option<String> {
        Some(
            match key {
                "LOCAL_ANSWER_EXECUTION_ENABLED" => "true",
                "LOCAL_ANSWER_ENDPOINT" => "http://127.0.0.1:11435",
                "LOCAL_ANSWER_MODEL" => "qwen3:4b-fixture",
                "LOCAL_ANSWER_GUARD_SCRIPT" => "/opt/project/scripts/local-model.mjs",
                _ => return None,
            }
            .into(),
        )
    }
    #[test]
    fn execution_is_disabled_by_default_and_never_reads_other_configuration_when_disabled() {
        assert!(settings(|_| None).unwrap().is_none());
        assert!(
            settings(|key| {
                assert_eq!(key, "LOCAL_ANSWER_EXECUTION_ENABLED");
                Some("false".into())
            })
            .unwrap()
            .is_none()
        );
    }
    #[test]
    fn enablement_requires_exact_loopback_target_and_explicit_fixed_guard_configuration() {
        let good = settings(configuration).unwrap().unwrap();
        assert_eq!(good.target, target());
        assert_eq!(good.node_binary, "node");
        for (key, bad) in [
            ("LOCAL_ANSWER_EXECUTION_ENABLED", "yes"),
            ("LOCAL_ANSWER_ENDPOINT", "http://192.168.1.1:11435"),
            (
                "LOCAL_ANSWER_ENDPOINT",
                "http://user:secret@127.0.0.1:11435",
            ),
            ("LOCAL_ANSWER_MODEL", "model-cloud"),
            ("LOCAL_ANSWER_GUARD_SCRIPT", "scripts/local-model.mjs"),
            ("LOCAL_ANSWER_GUARD_SCRIPT", ""),
            ("LOCAL_ANSWER_NODE_BINARY", "node --eval secret"),
        ] {
            let error = settings(|name| {
                if name == key {
                    Some(bad.into())
                } else {
                    configuration(name)
                }
            })
            .err()
            .unwrap();
            assert!(!error.contains("secret"));
        }
        for absent in [
            "LOCAL_ANSWER_ENDPOINT",
            "LOCAL_ANSWER_MODEL",
            "LOCAL_ANSWER_GUARD_SCRIPT",
        ] {
            assert!(
                settings(|name| if name == absent {
                    None
                } else {
                    configuration(name)
                })
                .is_err()
            );
        }
    }

    struct Guard {
        checks: AtomicUsize,
        denied: Option<usize>,
    }
    impl AnswerExecutionGuard for Guard {
        fn check(&self) -> BoxFuture<'_, Result<(), &'static str>> {
            Box::pin(async move {
                let index = self.checks.fetch_add(1, Ordering::SeqCst);
                if self.denied == Some(index) {
                    Err("private guard detail")
                } else {
                    Ok(())
                }
            })
        }
    }
    struct Provider(AtomicUsize);
    impl AnswerProvider for Provider {
        fn answer(&self, _: &str, _: &[AnswerSource]) -> BoxFuture<'_, LlmResult<ModelAnswer>> {
            Box::pin(async move {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(ModelAnswer {
                    answer: String::new(),
                    citations: vec![],
                    insufficient_evidence: true,
                })
            })
        }
    }
    fn fixture(denied: Option<usize>) -> (Arc<AnswerExecutionRuntime>, Arc<Guard>, Arc<Provider>) {
        let guard = Arc::new(Guard {
            checks: AtomicUsize::new(0),
            denied,
        });
        let provider = Arc::new(Provider(AtomicUsize::new(0)));
        (
            Arc::new(AnswerExecutionRuntime::new(
                target(),
                provider.clone(),
                guard.clone(),
            )),
            guard,
            provider,
        )
    }
    #[tokio::test]
    async fn denial_before_claim_or_before_send_cannot_invoke_provider_and_releases_slot() {
        for denied in [0, 1] {
            let (runtime, guard, provider) = fixture(Some(denied));
            assert_eq!(runtime.target(), &target());
            match runtime.admit().await {
                Err(error) => assert_eq!(error, GUARD_UNAVAILABLE),
                Ok(permit) => {
                    assert_eq!(runtime.admit().await.err(), Some("answer_execution_busy"));
                    assert_eq!(guard.checks.load(Ordering::SeqCst), 1);
                    let error = permit
                        .answer("question", &[])
                        .await
                        .unwrap_err()
                        .to_string();
                    assert!(!error.contains("private"));
                }
            }
            assert_eq!(provider.0.load(Ordering::SeqCst), 0);
            let permit = runtime.admit().await.unwrap();
            permit.answer("question", &[]).await.unwrap();
            assert_eq!(provider.0.load(Ordering::SeqCst), 1);
            assert_eq!(guard.checks.load(Ordering::SeqCst), denied + 3);
        }
    }
    #[tokio::test]
    async fn live_permission_is_rechecked_after_resource_admission_before_the_provider_send() {
        for denied in [None, Some(1)] {
            let (runtime, guard, provider) = fixture(denied);
            let permit = runtime.admit().await.unwrap();
            let rechecks = AtomicUsize::new(0);
            let before_send = Box::pin(async {
                assert_eq!(guard.checks.load(Ordering::SeqCst), 2);
                assert_eq!(provider.0.load(Ordering::SeqCst), 0);
                rechecks.fetch_add(1, Ordering::SeqCst);
                Err(LlmError::ProviderUnavailable("permission revoked".into()))
            });
            assert!(
                permit
                    .answer_checked("question", &[], before_send)
                    .await
                    .is_err()
            );
            assert_eq!(
                rechecks.load(Ordering::SeqCst),
                usize::from(denied.is_none())
            );
            assert_eq!(provider.0.load(Ordering::SeqCst), 0);
            assert!(runtime.admit().await.is_ok());
        }
    }
    struct PausedProvider {
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }
    impl AnswerProvider for PausedProvider {
        fn answer(&self, _: &str, _: &[AnswerSource]) -> BoxFuture<'_, LlmResult<ModelAnswer>> {
            Box::pin(async move {
                self.started.notify_one();
                self.release.notified().await;
                Err(LlmError::ProviderUnavailable("fixture".into()))
            })
        }
    }
    #[tokio::test]
    async fn slot_remains_exclusive_during_send_and_is_released_on_cancellation() {
        let provider = Arc::new(PausedProvider {
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let runtime = Arc::new(AnswerExecutionRuntime::new(
            target(),
            provider.clone(),
            Arc::new(Guard {
                checks: AtomicUsize::new(0),
                denied: None,
            }),
        ));
        let permit = runtime.admit().await.unwrap();
        let sending = tokio::spawn(async move { permit.answer("question", &[]).await });
        provider.started.notified().await;
        assert_eq!(runtime.admit().await.err(), Some("answer_execution_busy"));
        sending.abort();
        assert!(sending.await.is_err());
        assert!(runtime.admit().await.is_ok());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn guard_command_uses_fixed_separate_arguments_and_rejects_failure_or_timeout() {
        let directory = std::env::temp_dir().join(format!("answer-guard-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let script = directory.join("guard fixture.mjs");
        std::fs::write(
            &script,
            "test \"$#\" -eq 3 && test \"$1\" = answer-admit && test \"$2\" = http://127.0.0.1:11435 && test \"$3\" = qwen3:4b-fixture && test -z \"${DATABASE_URL+x}\" && test -z \"${OPENAI_API_KEY+x}\" && test -z \"${API_AUTH_TOKEN+x}\"\n",
        )
        .unwrap();
        let guard = CommandGuard {
            script: script.clone(),
            node_binary: "/bin/sh".into(),
            target: target(),
            timeout: Duration::from_secs(1),
        };
        guard.check().await.unwrap();
        std::fs::write(&script, "printf private-guard-detail >&2\nexit 1\n").unwrap();
        assert_eq!(guard.check().await, Err(GUARD_UNAVAILABLE));
        std::fs::write(&script, "exec sleep 5\n").unwrap();
        let guard = CommandGuard {
            timeout: Duration::from_millis(25),
            ..guard
        };
        assert_eq!(guard.check().await, Err(GUARD_UNAVAILABLE));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
