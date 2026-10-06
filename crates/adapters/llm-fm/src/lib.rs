//! `FoundationModelsProvider`: the `LlmProvider` backed by Apple Foundation Models, the on-device
//! model of macOS 27, through the system tool `/usr/bin/fm` — no Swift (ADR 0002).
//!
//! - compatibility (macOS 27+, Apple Silicon, native, `fm` present) is checked before anything;
//! - availability comes from `fm available` (classified, cached for a few seconds);
//! - exact token counts from `fm count-tokens`;
//! - generation through a supervised `fm serve --socket` (HTTP/SSE over a Unix socket), falling
//!   back to `fm respond --stream` when the server can't start; both stream and can be cancelled.

mod compat;
mod errors;
mod respond;
mod serve;
mod sse;

use std::{
    path::PathBuf,
    process::Stdio,
    sync::Mutex as StdMutex,
    time::{Duration, Instant},
};

use nlmx_application::ports::{BoxFuture, CancelFlag, LlmProvider};
use nlmx_domain::generation::{
    FinishReason, Generation, GenerationRequest, LanguageModelStatus, LlmCapabilities, LlmError,
};
use tokio::{
    process::Command,
    sync::{Mutex, Semaphore},
};

pub use self::{
    compat::{Incompatibility, MIN_MACOS, SystemInfo},
    serve::{FoundationModelsConfig, MAX_SOCKET_PATH},
};
use self::{
    errors::{EXIT_LICENSE_NOT_ACCEPTED, clean},
    sse::{SseEvent, SseParser},
};

const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const COUNT_TIMEOUT: Duration = Duration::from_secs(10);
/// The on-device system model's context window.
pub const SYSTEM_CONTEXT_TOKENS: u32 = 4096;
pub(crate) const CANCEL_POLL: Duration = Duration::from_millis(100);

pub struct FoundationModelsProvider {
    config: FoundationModelsConfig,
    /// Checked once: the OS, architecture and binary don't change while the app runs.
    compatibility: Result<(), Incompatibility>,
    status: StdMutex<Option<(Instant, LanguageModelStatus)>>,
    /// Serializes probes: concurrent re-checks share one `fm available` run.
    probing: Mutex<()>,
    server: Mutex<Option<serve::Running>>,
    /// Set when `fm serve` failed to start; generation uses `fm respond` until it expires.
    serve_failed_at: StdMutex<Option<Instant>>,
    /// One generation at a time (the on-device model serves one request well).
    generation: Semaphore,
}

impl FoundationModelsProvider {
    pub fn new(config: FoundationModelsConfig) -> Self {
        Self::with_system(config, &SystemInfo::current())
    }

    /// With explicit system facts (tests).
    pub fn with_system(config: FoundationModelsConfig, system: &SystemInfo) -> Self {
        let compatibility = compat::check(system, &config.binary);
        if let Err(reason) = &compatibility {
            tracing::warn!(reason = %reason.message(), "Apple Foundation Models incompatible");
        }
        Self {
            config,
            compatibility,
            status: StdMutex::new(None),
            probing: Mutex::new(()),
            server: Mutex::new(None),
            serve_failed_at: StdMutex::new(None),
            generation: Semaphore::new(1),
        }
    }

    /// The `fm` shipped with macOS 27, with its pidfile and log in `run_dir`.
    pub fn system(run_dir: impl Into<PathBuf>) -> Self {
        Self::new(FoundationModelsConfig::new("/usr/bin/fm", run_dir))
    }

    /// Stops `fm serve` if it is running. Idempotent.
    pub async fn shutdown(&self) {
        if let Some(running) = self.server.lock().await.take() {
            serve::stop(&self.config, running).await;
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.config.binary);
        command
            .env("NO_COLOR", "1")
            .envs(self.config.extra_env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .kill_on_drop(true);
        command
    }

    async fn current_status(&self) -> LanguageModelStatus {
        if let Err(incompatibility) = &self.compatibility {
            return match incompatibility {
                Incompatibility::NotInstalled => LanguageModelStatus::NotInstalled,
                Incompatibility::Unsupported(reason) => LanguageModelStatus::Incompatible {
                    reason: reason.clone(),
                },
            };
        }
        if let Some(status) = self.fresh_cached() {
            return status;
        }
        let _probing = self.probing.lock().await;
        // Another caller may have probed while this one waited.
        if let Some(status) = self.fresh_cached() {
            return status;
        }
        let status = self.probe().await;
        *self.status.lock().unwrap() = Some((Instant::now(), status.clone()));
        status
    }

    fn fresh_cached(&self) -> Option<LanguageModelStatus> {
        let cached = self.status.lock().unwrap();
        let (at, status) = cached.as_ref()?;
        (at.elapsed() < self.config.status_ttl).then(|| status.clone())
    }

    /// Asks `fm` again even if the cache is fresh. Callers that arrive while a probe is
    /// running share its answer instead of starting another.
    async fn recheck_status(&self) -> LanguageModelStatus {
        let started = Instant::now();
        let waiting = self.probing.try_lock().is_err();
        if waiting {
            // A probe is in flight: wait for it, then use what it stored.
            drop(self.probing.lock().await);
            if let Some((at, status)) = self.status.lock().unwrap().as_ref() {
                if *at >= started {
                    return status.clone();
                }
            }
        }
        self.forget_status();
        self.current_status().await
    }

    fn forget_status(&self) {
        *self.status.lock().unwrap() = None;
    }

    async fn probe(&self) -> LanguageModelStatus {
        let output = self
            .command()
            .args(["available", "--model", "system"])
            .output();
        let output = match tokio::time::timeout(STATUS_TIMEOUT, output).await {
            Err(_) => {
                return unavailable("O Apple Foundation Models não respondeu a tempo.");
            }
            Ok(Err(error)) => {
                // The details stay in the logs; the user only sees a generic message.
                tracing::warn!(%error, "could not run fm available");
                return unavailable("Não foi possível verificar o Apple Foundation Models.");
            }
            Ok(Ok(output)) => output,
        };
        match output.status.code() {
            Some(0) => LanguageModelStatus::Available,
            Some(EXIT_LICENSE_NOT_ACCEPTED) => LanguageModelStatus::LicenseRequired,
            _ => {
                let raw = if output.stderr.is_empty() {
                    &output.stdout
                } else {
                    &output.stderr
                };
                let reason = clean(&String::from_utf8_lossy(raw));
                LanguageModelStatus::Unavailable {
                    kind: errors::classify(&reason),
                    reason: if reason.is_empty() {
                        "O Apple Foundation Models está indisponível neste Mac.".into()
                    } else {
                        reason
                    },
                }
            }
        }
    }

    /// Fails fast (without spawning `fm`) when the system can't run the model.
    fn ensure_compatible(&self) -> Result<(), LlmError> {
        self.compatibility
            .clone()
            .map_err(|incompatibility| LlmError::Unavailable(incompatibility.message()))
    }

    async fn count(&self, request: &GenerationRequest) -> Result<u32, LlmError> {
        self.ensure_compatible()?;
        let output = self
            .command()
            .args(["count-tokens", "-q", "-i"])
            .arg(&request.system)
            .arg(request.flat_user())
            .output();
        let output = tokio::time::timeout(COUNT_TIMEOUT, output)
            .await
            .map_err(|_| LlmError::Timeout)?
            .map_err(|e| LlmError::Unavailable(e.to_string()))?;
        if !output.status.success() {
            return Err(errors::from_exit(
                output.status.code(),
                &String::from_utf8_lossy(&output.stderr),
            ));
        }
        let text = clean(&String::from_utf8_lossy(&output.stdout));
        text.parse()
            .map_err(|_| LlmError::Protocol(format!("contagem de tokens inválida: {text:?}")))
    }

    /// A healthy `fm serve`: started lazily, health-checked after idling, restarted if it died.
    async fn client(&self) -> Result<reqwest::Client, LlmError> {
        let mut slot = self.server.lock().await;
        if let Some(running) = slot.as_mut() {
            let healthy = running.is_alive()
                && (running.last_used.elapsed() < self.config.health_after_idle
                    || serve::health(&running.client, Duration::from_secs(2)).await);
            if healthy {
                running.last_used = Instant::now();
                return Ok(running.client.clone());
            }
            tracing::warn!("fm serve unhealthy; restarting");
            if let Some(dead) = slot.take() {
                serve::stop(&self.config, dead).await;
            }
        }
        let running = serve::start(&self.config).await?;
        let client = running.client.clone();
        *slot = Some(running);
        Ok(client)
    }

    fn serve_recently_failed(&self) -> bool {
        self.serve_failed_at
            .lock()
            .unwrap()
            .is_some_and(|at| at.elapsed() < self.config.serve_retry_after)
    }

    async fn generate_any(
        &self,
        request: &GenerationRequest,
        on_token: &(dyn Fn(&str) + Send + Sync),
        cancel: CancelFlag,
    ) -> Result<Generation, LlmError> {
        self.ensure_compatible()?;
        let _turn = self
            .generation
            .acquire()
            .await
            .map_err(|_| LlmError::Unavailable("encerrando".into()))?;
        if cancel.is_cancelled() {
            return Ok(Generation {
                text: String::new(),
                finish: FinishReason::Cancelled,
            });
        }
        let result = if self.config.respond_only || self.serve_recently_failed() {
            respond::generate(&self.config, request, on_token, cancel).await
        } else {
            match self.client().await {
                Ok(client) => match self
                    .stream(&client, request, on_token, cancel.clone())
                    .await
                {
                    // The server went away before answering: one attempt through `fm respond`.
                    Err(LlmError::Unavailable(reason)) => {
                        tracing::warn!(%reason, "fm serve failed; using fm respond");
                        respond::generate(&self.config, request, on_token, cancel).await
                    }
                    other => other,
                },
                Err(LlmError::LicenseRequired) => Err(LlmError::LicenseRequired),
                Err(e) => {
                    tracing::warn!(error = %e, "fm serve did not start; using fm respond");
                    *self.serve_failed_at.lock().unwrap() = Some(Instant::now());
                    respond::generate(&self.config, request, on_token, cancel).await
                }
            }
        };
        if matches!(
            result,
            Err(LlmError::LicenseRequired | LlmError::Unavailable(_))
        ) {
            // Availability changed: re-probe on the next status call.
            self.forget_status();
        }
        result
    }

    async fn stream(
        &self,
        client: &reqwest::Client,
        request: &GenerationRequest,
        on_token: &(dyn Fn(&str) + Send + Sync),
        cancel: CancelFlag,
    ) -> Result<Generation, LlmError> {
        let mut messages = vec![serde_json::json!({ "role": "system", "content": request.system })];
        for turn in &request.history {
            messages.push(serde_json::json!({ "role": "user", "content": turn.user }));
            messages.push(serde_json::json!({ "role": "assistant", "content": turn.assistant }));
        }
        messages.push(serde_json::json!({ "role": "user", "content": request.user }));
        let body = serde_json::json!({
            "model": "system",
            "stream": true,
            "temperature": request.temperature,
            "max_tokens": request.max_tokens,
            "messages": messages,
        });
        let send = client
            .post("http://localhost/v1/chat/completions")
            .json(&body)
            .send();
        let mut response = tokio::time::timeout(self.config.idle_timeout, send)
            .await
            .map_err(|_| LlmError::Timeout)?
            .map_err(|e| LlmError::Unavailable(format!("fm serve: {e}")))?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            let message = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(String::from))
                .unwrap_or(text);
            return Err(match errors::from_message(&message) {
                LlmError::Protocol(m) => LlmError::Protocol(format!("HTTP {status}: {m}")),
                other => other,
            });
        }

        let mut parser = SseParser::default();
        let mut text = String::new();
        let mut finish = FinishReason::Completed;
        let mut idle_since = Instant::now();
        loop {
            if cancel.is_cancelled() {
                // Dropping the response closes the connection and stops the generation.
                return Ok(Generation {
                    text,
                    finish: FinishReason::Cancelled,
                });
            }
            let chunk = match tokio::time::timeout(CANCEL_POLL, response.chunk()).await {
                Err(_) => {
                    if idle_since.elapsed() > self.config.idle_timeout {
                        return Err(LlmError::Timeout);
                    }
                    continue;
                }
                Ok(Err(e)) => return Err(LlmError::Protocol(e.to_string())),
                Ok(Ok(None)) => break,
                Ok(Ok(Some(bytes))) => bytes,
            };
            idle_since = Instant::now();
            for event in parser.push(&chunk).map_err(LlmError::Protocol)? {
                match event {
                    SseEvent::Delta(piece) => {
                        on_token(&piece);
                        text.push_str(&piece);
                    }
                    SseEvent::Finish(reason) => {
                        if reason == "length" {
                            finish = FinishReason::Length;
                        }
                    }
                    SseEvent::Error(message) => return Err(errors::from_message(&message)),
                    SseEvent::Done => return Ok(Generation { text, finish }),
                }
            }
        }
        Ok(Generation { text, finish })
    }
}

impl LlmProvider for FoundationModelsProvider {
    fn status(&self) -> BoxFuture<'_, LanguageModelStatus> {
        Box::pin(self.current_status())
    }

    fn recheck(&self) -> BoxFuture<'_, LanguageModelStatus> {
        Box::pin(self.recheck_status())
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            context_tokens: SYSTEM_CONTEXT_TOKENS,
        }
    }

    fn count_tokens<'a>(
        &'a self,
        request: &'a GenerationRequest,
    ) -> BoxFuture<'a, Result<u32, LlmError>> {
        Box::pin(self.count(request))
    }

    fn generate<'a>(
        &'a self,
        request: &'a GenerationRequest,
        on_token: &'a (dyn Fn(&str) + Send + Sync),
        cancel: CancelFlag,
    ) -> BoxFuture<'a, Result<Generation, LlmError>> {
        Box::pin(self.generate_any(request, on_token, cancel))
    }
}

fn unavailable(reason: &str) -> LanguageModelStatus {
    LanguageModelStatus::Unavailable {
        kind: errors::classify(reason),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt, path::Path};

    use nlmx_domain::generation::UnavailableKind;

    use super::*;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn provider(binary: PathBuf, dir: &Path) -> FoundationModelsProvider {
        FoundationModelsProvider::new(FoundationModelsConfig::new(binary, dir.join("run")))
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nlmx-llm-fm-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn exit_zero_is_available() {
        let dir = temp_dir("ok");
        let fm = provider(script(&dir, "fm", "echo 'System model available'"), &dir);
        assert_eq!(fm.status().await, LanguageModelStatus::Available);
    }

    #[tokio::test]
    async fn exit_69_requires_license() {
        let dir = temp_dir("license");
        let fm = provider(script(&dir, "fm", "exit 69"), &dir);
        assert_eq!(fm.status().await, LanguageModelStatus::LicenseRequired);
    }

    #[tokio::test]
    async fn other_failures_are_classified_without_ansi() {
        let dir = temp_dir("fail");
        let fm = provider(
            script(
                &dir,
                "fm",
                r"printf '\033[31mError: Apple Intelligence is not enabled\033[0m' >&2; exit 1",
            ),
            &dir,
        );
        assert_eq!(
            fm.status().await,
            LanguageModelStatus::Unavailable {
                kind: UnavailableKind::AppleIntelligenceDisabled,
                reason: "Apple Intelligence is not enabled".into()
            }
        );
    }

    #[tokio::test]
    async fn missing_binary_is_not_installed_and_never_spawned() {
        let fm = provider("/nonexistent/fm".into(), &temp_dir("missing"));
        assert_eq!(fm.status().await, LanguageModelStatus::NotInstalled);
        assert_eq!(fm.recheck().await, LanguageModelStatus::NotInstalled);
        let request = GenerationRequest {
            system: "s".into(),
            history: Vec::new(),
            user: "u".into(),
            temperature: 0.2,
            max_tokens: 10,
        };
        assert!(matches!(
            fm.generate(&request, &|_| {}, CancelFlag::default()).await,
            Err(LlmError::Unavailable(_))
        ));
        assert!(matches!(
            fm.count_tokens(&request).await,
            Err(LlmError::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn an_old_macos_is_incompatible() {
        let dir = temp_dir("old");
        let binary = script(&dir, "fm", "echo 'System model available'");
        let fm = FoundationModelsProvider::with_system(
            FoundationModelsConfig::new(binary, dir.join("run")),
            &SystemInfo {
                os_version: Some("26.4".into()),
                arch: "aarch64".into(),
                translated: false,
            },
        );
        match fm.status().await {
            LanguageModelStatus::Incompatible { reason } => assert!(reason.contains("macOS 27")),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn status_is_cached() {
        let dir = temp_dir("cache");
        let calls = dir.join("calls");
        let _ = fs::remove_file(&calls);
        let fm = provider(
            script(&dir, "fm", &format!("echo x >> '{}'", calls.display())),
            &dir,
        );
        assert!(fm.status().await.is_available());
        assert!(fm.status().await.is_available());
        assert_eq!(fs::read_to_string(&calls).unwrap().lines().count(), 1);
        fm.forget_status();
        fm.status().await;
        assert_eq!(fs::read_to_string(&calls).unwrap().lines().count(), 2);
    }

    #[tokio::test]
    async fn recheck_ignores_the_cache_and_sees_the_license_accepted() {
        let dir = temp_dir("recheck");
        let accepted = dir.join("accepted");
        let _ = fs::remove_file(&accepted);
        let fm = provider(
            script(
                &dir,
                "fm",
                &format!("[ -f '{}' ] || exit 69", accepted.display()),
            ),
            &dir,
        );
        assert_eq!(fm.status().await, LanguageModelStatus::LicenseRequired);
        fs::write(&accepted, "").unwrap();
        // Still cached…
        assert_eq!(fm.status().await, LanguageModelStatus::LicenseRequired);
        // …but a re-check asks `fm` again.
        assert_eq!(fm.recheck().await, LanguageModelStatus::Available);
        assert_eq!(fm.status().await, LanguageModelStatus::Available);
        fs::remove_file(&accepted).unwrap();
        assert_eq!(fm.recheck().await, LanguageModelStatus::LicenseRequired);
    }

    #[tokio::test]
    async fn simultaneous_rechecks_run_one_probe() {
        let dir = temp_dir("single-flight");
        let calls = dir.join("calls");
        let _ = fs::remove_file(&calls);
        let fm = provider(
            script(
                &dir,
                "fm",
                &format!("echo x >> '{}'; sleep 0.5; exit 69", calls.display()),
            ),
            &dir,
        );
        let (a, b, c) = tokio::join!(fm.recheck(), fm.recheck(), fm.recheck());
        assert_eq!(a, LanguageModelStatus::LicenseRequired);
        assert_eq!(a, b);
        assert_eq!(b, c);
        assert_eq!(fs::read_to_string(&calls).unwrap().lines().count(), 1);
    }

    #[tokio::test]
    async fn a_failure_to_run_fm_does_not_leak_paths() {
        let dir = temp_dir("spawn-fails");
        let binary = dir.join("fm");
        fs::write(&binary, "#!/nonexistent/interpreter\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        match provider(binary, &dir).status().await {
            LanguageModelStatus::Unavailable { reason, .. } => {
                assert!(!reason.contains(dir.to_str().unwrap()), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn real_fm_reports_a_status() {
        // Smoke test against the system binary when present; any variant is acceptable.
        if Path::new("/usr/bin/fm").exists() {
            let _ = FoundationModelsProvider::system(temp_dir("real"))
                .status()
                .await;
        }
    }
}
