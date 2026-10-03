//! Supervises `fm serve --socket <path>`: lazy start, health check, restart with backoff,
//! orphan cleanup (pidfile) and shutdown. Never uses TCP mode.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use nlmx_domain::generation::LlmError;
use tokio::process::{Child, Command};

/// `sun_path` holds 104 bytes on macOS; a longer path fails silently.
pub const MAX_SOCKET_PATH: usize = 103;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const START_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone)]
pub struct FoundationModelsConfig {
    pub binary: PathBuf,
    /// Pidfile and log live here (`<data>/run`).
    pub run_dir: PathBuf,
    /// Directory of the socket; must keep the full path under 104 bytes.
    pub socket_dir: PathBuf,
    pub startup_timeout: Duration,
    /// Max wait between two pieces of the streamed answer.
    pub idle_timeout: Duration,
    /// Grace period between SIGTERM and SIGKILL.
    pub stop_timeout: Duration,
    /// Health check before a generation when idle for this long.
    pub health_after_idle: Duration,
    /// How long a status probe (`fm available`) is reused.
    pub status_ttl: Duration,
    /// After `fm serve` fails to start, use `fm respond` for this long before trying again.
    pub serve_retry_after: Duration,
    /// Always generate through `fm respond` (diagnostics and tests).
    pub respond_only: bool,
    /// Extra environment for the child process (used by tests).
    pub extra_env: Vec<(String, String)>,
    /// Distinguishes providers within one process (one socket each).
    instance: u32,
}

static INSTANCES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

impl FoundationModelsConfig {
    pub fn new(binary: impl Into<PathBuf>, run_dir: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            run_dir: run_dir.into(),
            socket_dir: std::env::temp_dir(),
            startup_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(60),
            stop_timeout: Duration::from_secs(2),
            health_after_idle: Duration::from_secs(60),
            status_ttl: Duration::from_secs(30),
            serve_retry_after: Duration::from_secs(300),
            respond_only: false,
            extra_env: Vec::new(),
            instance: INSTANCES.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    pub fn socket_path(&self) -> PathBuf {
        let name = format!("nlmx-fm-{}-{}.sock", std::process::id(), self.instance);
        let preferred = self.socket_dir.join(&name);
        if preferred.as_os_str().len() <= MAX_SOCKET_PATH {
            preferred
        } else {
            PathBuf::from("/tmp").join(name)
        }
    }

    fn pidfile(&self) -> PathBuf {
        self.run_dir.join("fm.pid")
    }

    fn log_path(&self) -> PathBuf {
        self.run_dir.join("fm-serve.log")
    }
}

pub struct Running {
    child: Child,
    pub socket: PathBuf,
    pub client: reqwest::Client,
    pub last_used: Instant,
}

impl Running {
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

pub async fn health(client: &reqwest::Client, timeout: Duration) -> bool {
    matches!(
        tokio::time::timeout(timeout, client.get("http://localhost/health").send()).await,
        Ok(Ok(r)) if r.status().is_success()
    )
}

/// Starts the server, retrying with exponential backoff.
pub async fn start(config: &FoundationModelsConfig) -> Result<Running, LlmError> {
    clean_orphan(config).await;
    let mut delay = Duration::from_millis(500);
    let mut last = LlmError::Unavailable("o fm serve não iniciou".into());
    for attempt in 1..=START_ATTEMPTS {
        match start_once(config).await {
            Ok(running) => return Ok(running),
            Err(e @ LlmError::LicenseRequired) => return Err(e),
            Err(e) => {
                tracing::warn!(attempt, error = %e, "fm serve failed to start");
                last = e;
            }
        }
        if attempt < START_ATTEMPTS {
            tokio::time::sleep(delay).await;
            delay *= 2;
        }
    }
    Err(last)
}

async fn start_once(config: &FoundationModelsConfig) -> Result<Running, LlmError> {
    let socket = config.socket_path();
    assert!(
        socket.as_os_str().len() <= MAX_SOCKET_PATH,
        "socket path over 104 bytes: {}",
        socket.display()
    );
    let _ = std::fs::remove_file(&socket);
    std::fs::create_dir_all(&config.run_dir)
        .map_err(|e| LlmError::Unavailable(format!("pasta de execução: {e}")))?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config.log_path())
        .map_err(|e| LlmError::Unavailable(format!("log do fm: {e}")))?;
    let mut command = Command::new(&config.binary);
    command
        .arg("serve")
        .arg("--socket")
        .arg(&socket)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(
            log.try_clone()
                .map_err(|e| LlmError::Protocol(e.to_string()))?,
        )
        .stderr(log)
        .kill_on_drop(true);
    for (k, v) in &config.extra_env {
        command.env(k, v);
    }
    let mut child = command
        .spawn()
        .map_err(|e| LlmError::Unavailable(format!("não foi possível iniciar o fm: {e}")))?;
    if let Some(pid) = child.id() {
        let _ = std::fs::write(config.pidfile(), pid.to_string());
    }
    let client = reqwest::Client::builder()
        .unix_socket(socket.clone())
        .build()
        .map_err(|e| LlmError::Protocol(e.to_string()))?;

    let deadline = Instant::now() + config.startup_timeout;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let _ = std::fs::remove_file(config.pidfile());
            return Err(match status.code() {
                Some(69) => LlmError::LicenseRequired,
                code => LlmError::Unavailable(format!("o fm serve terminou (código {code:?})")),
            });
        }
        if socket.exists() && health(&client, Duration::from_secs(2)).await {
            return Ok(Running {
                child,
                socket,
                client,
                last_used: Instant::now(),
            });
        }
        if Instant::now() >= deadline {
            let _ = child.kill().await;
            let _ = std::fs::remove_file(&socket);
            return Err(LlmError::Timeout);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// SIGTERM, then SIGKILL after the grace period; removes the socket and the pidfile.
pub async fn stop(config: &FoundationModelsConfig, mut running: Running) {
    if let Some(pid) = running.child.id() {
        signal(pid, "TERM");
        if tokio::time::timeout(config.stop_timeout, running.child.wait())
            .await
            .is_err()
        {
            let _ = running.child.kill().await;
        }
    }
    let _ = std::fs::remove_file(&running.socket);
    let _ = std::fs::remove_file(config.pidfile());
}

/// A server left by a previous session that crashed: stopped only if it is our `fm serve`.
async fn clean_orphan(config: &FoundationModelsConfig) {
    let Some(pid) = std::fs::read_to_string(config.pidfile())
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    else {
        return;
    };
    if is_our_server(pid, &config.binary) {
        tracing::info!(pid, "stopping orphaned fm serve");
        signal(pid, "TERM");
        let deadline = Instant::now() + config.stop_timeout;
        while is_our_server(pid, &config.binary) {
            if Instant::now() >= deadline {
                signal(pid, "KILL");
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
    let _ = std::fs::remove_file(config.pidfile());
}

/// Whether `pid` is alive and runs `<binary> serve` (never signal someone else's process).
fn is_our_server(pid: u32, binary: &Path) -> bool {
    let Ok(output) = std::process::Command::new("/bin/ps")
        .args([
            "-ww",
            "-p",
            &pid.to_string(),
            "-o",
            "stat=",
            "-o",
            "command=",
        ])
        .output()
    else {
        return false;
    };
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let Some((stat, command)) = line.split_once(char::is_whitespace) else {
        return false;
    };
    output.status.success()
        && !stat.starts_with('Z')
        && command
            .trim()
            .starts_with(&format!("{} serve", binary.display()))
}

fn signal(pid: u32, signal: &str) {
    let _ = std::process::Command::new("/bin/kill")
        .args([&format!("-{signal}"), &pid.to_string()])
        .status();
}
