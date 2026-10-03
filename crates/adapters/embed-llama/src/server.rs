//! Supervises a `llama-server` child process bound to 127.0.0.1 (ADR 0006).

use std::{
    io::Read,
    net::{Ipv4Addr, TcpListener},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use nlmx_domain::embedding::EmbeddingError;
use tokio::{process::Child, sync::Mutex};

use crate::{
    logs::LogSink,
    pidfile::{self, PidRecord},
};

const POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub struct LlamaServerConfig {
    pub binary: PathBuf,
    pub model_path: PathBuf,
    pub pooling: String,
    pub context_size: u32,
    pub batch_size: u32,
    pub gpu_layers: u32,
    /// Holds the pidfile and the API key file.
    pub run_dir: PathBuf,
    pub log_path: PathBuf,
    /// Model loading can take a while on first run.
    pub startup_timeout: Duration,
    pub health_timeout: Duration,
    /// Grace period between SIGTERM and SIGKILL.
    pub stop_timeout: Duration,
    /// Stop the server after this long without requests (it starts again on the next one);
    /// `None` keeps it running until `stop`.
    pub idle_shutdown: Option<Duration>,
    /// Extra environment variables for the server process.
    pub extra_env: Vec<(String, String)>,
}

/// Default for `LlamaServerConfig::idle_shutdown`.
pub const DEFAULT_IDLE_SHUTDOWN: Duration = Duration::from_secs(45);

impl LlamaServerConfig {
    pub fn new(binary: PathBuf, model_path: PathBuf, data_dir: &std::path::Path) -> Self {
        Self {
            binary,
            model_path,
            pooling: "mean".into(),
            context_size: 8192,
            batch_size: 2048,
            gpu_layers: 99,
            run_dir: data_dir.join("run"),
            log_path: data_dir.join("logs").join("llama-server.log"),
            startup_timeout: Duration::from_secs(120),
            health_timeout: Duration::from_secs(2),
            stop_timeout: Duration::from_secs(5),
            idle_shutdown: Some(DEFAULT_IDLE_SHUTDOWN),
            extra_env: Vec::new(),
        }
    }

    fn pidfile(&self) -> PathBuf {
        self.run_dir.join("llama-server.json")
    }
}

/// Where and how to reach the running server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub base_url: String,
    pub api_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    Ready,
    /// Running but still loading the model.
    Loading,
    Down(String),
}

struct Running {
    pid: u32,
    endpoint: Endpoint,
    /// `None` when an existing server from a previous session was adopted.
    child: Option<Child>,
}

pub struct LlamaServer {
    config: LlamaServerConfig,
    state: Mutex<Option<Running>>,
    logs: Arc<LogSink>,
    http: reqwest::Client,
    /// Bumped by every `begin_use`; an idle stop only happens if it did not change meanwhile.
    activity: AtomicU64,
    /// Uses in progress (`UseGuard`s alive).
    busy: AtomicUsize,
}

/// Marks the server as in use; dropping the last one schedules the idle stop.
pub struct UseGuard {
    server: Arc<LlamaServer>,
}

impl Drop for UseGuard {
    fn drop(&mut self) {
        if self.server.busy.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.server.schedule_idle_stop();
        }
    }
}

impl LlamaServer {
    pub fn new(config: LlamaServerConfig) -> Self {
        let logs = Arc::new(LogSink::new(config.log_path.clone()));
        let http = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("HTTP client");
        Self {
            config,
            state: Mutex::new(None),
            logs,
            http,
            activity: AtomicU64::new(0),
            busy: AtomicUsize::new(0),
        }
    }

    /// Call before using the server and keep the guard until done: while any guard is alive
    /// the server is never stopped for being idle.
    pub fn begin_use(self: &Arc<Self>) -> UseGuard {
        self.busy.fetch_add(1, Ordering::SeqCst);
        self.activity.fetch_add(1, Ordering::SeqCst);
        UseGuard {
            server: Arc::clone(self),
        }
    }

    /// Stops the server after `idle_shutdown` unless it is used again before then. The memory
    /// llama.cpp keeps after a burst of requests is only returned when the process exits.
    fn schedule_idle_stop(self: &Arc<Self>) {
        let Some(idle) = self.config.idle_shutdown else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let generation = self.activity.load(Ordering::SeqCst);
        let server: Weak<Self> = Arc::downgrade(self);
        runtime.spawn(async move {
            tokio::time::sleep(idle).await;
            if let Some(server) = server.upgrade() {
                server.stop_if_idle(generation).await;
            }
        });
    }

    async fn stop_if_idle(&self, generation: u64) {
        // Checked under the state lock: a new use bumps `activity` before `ensure_running`
        // takes the lock, so it either cancels this stop or starts a fresh server after it.
        let mut state = self.state.lock().await;
        if self.activity.load(Ordering::SeqCst) != generation
            || self.busy.load(Ordering::SeqCst) != 0
        {
            return;
        }
        if let Some(running) = state.take() {
            tracing::info!(pid = running.pid, "llama-server stopped after idle");
            self.logs.write(&format!(
                "--- idle for {:?}",
                self.config.idle_shutdown.unwrap_or_default()
            ));
            self.terminate(running).await;
            pidfile::remove(&self.config.pidfile());
        }
    }

    pub fn config(&self) -> &LlamaServerConfig {
        &self.config
    }

    pub fn recent_logs(&self) -> Vec<String> {
        self.logs.recent()
    }

    pub fn log_path(&self) -> &std::path::Path {
        self.logs.path()
    }

    pub async fn pid(&self) -> Option<u32> {
        self.state.lock().await.as_ref().map(|r| r.pid)
    }

    /// Starts the server unless it is already running (idempotent).
    pub async fn start(&self) -> Result<Endpoint, EmbeddingError> {
        let mut state = self.state.lock().await;
        if let Some(running) = state.as_mut() {
            if self.check(running).await == Health::Ready {
                return Ok(running.endpoint.clone());
            }
        }
        if let Some(stale) = state.take() {
            self.terminate(stale).await;
        }
        let running = match self.adopt_existing().await {
            Some(adopted) => adopted,
            None => self.spawn().await?,
        };
        let endpoint = running.endpoint.clone();
        *state = Some(running);
        Ok(endpoint)
    }

    /// Same as `start`; named for call sites that only need the server to be up.
    pub async fn ensure_running(&self) -> Result<Endpoint, EmbeddingError> {
        self.start().await
    }

    pub async fn health(&self) -> Health {
        let mut state = self.state.lock().await;
        match state.as_mut() {
            None => Health::Down("não iniciado".into()),
            Some(running) => self.check(running).await,
        }
    }

    /// Stops the server (SIGTERM, then SIGKILL after the grace period). Idempotent.
    pub async fn stop(&self) {
        if let Some(running) = self.state.lock().await.take() {
            self.terminate(running).await;
        }
        pidfile::remove(&self.config.pidfile());
    }

    pub async fn restart(&self) -> Result<Endpoint, EmbeddingError> {
        self.stop().await;
        self.start().await
    }

    async fn check(&self, running: &mut Running) -> Health {
        if let Some(child) = running.child.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                return Health::Down(format!("o processo terminou ({status})"));
            }
        } else if !pidfile::is_our_server(running.pid, &self.config.binary) {
            return Health::Down("o processo não existe mais".into());
        }
        self.probe(&running.endpoint.base_url).await
    }

    async fn probe(&self, base_url: &str) -> Health {
        let request = self
            .http
            .get(format!("{base_url}/health"))
            .timeout(self.config.health_timeout);
        match request.send().await {
            Ok(response) if response.status().is_success() => Health::Ready,
            Ok(response) if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE => {
                Health::Loading
            }
            Ok(response) => Health::Down(format!("health respondeu {}", response.status())),
            Err(err) => Health::Down(err.to_string()),
        }
    }

    /// Reuses a healthy server left by a previous session with the same binary and model;
    /// cleans up anything else recorded in the pidfile without touching foreign processes.
    async fn adopt_existing(&self) -> Option<Running> {
        let path = self.config.pidfile();
        let record = pidfile::read(&path)?;
        if !pidfile::is_our_server(record.pid, &self.config.binary) {
            tracing::info!(pid = record.pid, "stale llama-server pidfile removed");
            pidfile::remove(&path);
            return None;
        }
        let api_key = std::fs::read_to_string(pidfile::key_path(&self.config.run_dir)).ok();
        let endpoint = api_key.map(|key| Endpoint {
            base_url: format!("http://127.0.0.1:{}", record.port),
            api_key: key.trim().to_string(),
        });
        let same_model = record.model_path == self.config.model_path;
        if let Some(endpoint) = endpoint.filter(|_| same_model) {
            if self.probe(&endpoint.base_url).await == Health::Ready
                && self.authorized(&endpoint).await
            {
                tracing::info!(
                    pid = record.pid,
                    port = record.port,
                    "reusing running llama-server"
                );
                return Some(Running {
                    pid: record.pid,
                    endpoint,
                    child: None,
                });
            }
        }
        tracing::info!(
            pid = record.pid,
            same_model,
            "replacing previous llama-server"
        );
        self.kill_pid(record.pid).await;
        pidfile::remove(&path);
        None
    }

    async fn authorized(&self, endpoint: &Endpoint) -> bool {
        self.http
            .get(format!("{}/v1/models", endpoint.base_url))
            .bearer_auth(&endpoint.api_key)
            .timeout(self.config.health_timeout)
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
    }

    async fn spawn(&self) -> Result<Running, EmbeddingError> {
        let c = &self.config;
        if !c.binary.is_file() {
            return Err(EmbeddingError::Unavailable(format!(
                "llama-server não encontrado em {}",
                c.binary.display()
            )));
        }
        if !c.model_path.is_file() {
            return Err(EmbeddingError::Unavailable(format!(
                "modelo não encontrado em {}",
                c.model_path.display()
            )));
        }
        let port = free_port()
            .map_err(|err| EmbeddingError::Unavailable(format!("sem porta livre: {err}")))?;
        let api_key = random_key()
            .map_err(|err| EmbeddingError::Unavailable(format!("falha ao gerar a chave: {err}")))?;
        let key_file = pidfile::key_path(&c.run_dir);
        write_secret(&key_file, &api_key).map_err(|err| {
            EmbeddingError::Unavailable(format!("falha ao gravar a chave: {err}"))
        })?;

        self.logs.write(&format!(
            "--- starting {} on 127.0.0.1:{port}",
            c.binary.display()
        ));
        let mut command = tokio::process::Command::new(&c.binary);
        command
            .arg("--model")
            .arg(&c.model_path)
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            // The key goes in a 0600 file so it never shows up in `ps`.
            .arg("--api-key-file")
            .arg(&key_file)
            .args(["--embedding", "--pooling", &c.pooling])
            .args(["--ctx-size", &c.context_size.to_string()])
            .args([
                "--batch-size",
                &c.batch_size.to_string(),
                "--ubatch-size",
                &c.batch_size.to_string(),
            ])
            .args(["--n-gpu-layers", &c.gpu_layers.to_string()])
            // Embeddings never repeat a prompt: without these, the server keeps up to 8 GiB of
            // prompt cache (`--cache-ram` default) for as long as it runs. One slot is enough,
            // since the provider sends one request at a time.
            .args(["--cache-ram", "0", "--no-cache-prompt", "--parallel", "1"])
            .args(["--no-webui", "--offline", "--log-colors", "off"])
            .envs(c.extra_env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command.spawn().map_err(|err| {
            EmbeddingError::Unavailable(format!("falha ao iniciar o llama-server: {err}"))
        })?;
        let pid = child.id().unwrap_or_default();
        if let Some(out) = child.stdout.take() {
            self.logs.follow(out);
        }
        if let Some(err) = child.stderr.take() {
            self.logs.follow(err);
        }

        let mut running = Running {
            pid,
            endpoint: Endpoint {
                base_url: format!("http://127.0.0.1:{port}"),
                api_key,
            },
            child: Some(child),
        };
        let deadline = Instant::now() + c.startup_timeout;
        loop {
            match self.check(&mut running).await {
                Health::Ready => break,
                Health::Down(reason)
                    if running
                        .child
                        .as_mut()
                        .is_some_and(|ch| matches!(ch.try_wait(), Ok(Some(_)))) =>
                {
                    // Give the log readers a moment to drain the last lines.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    return Err(EmbeddingError::Unavailable(format!(
                        "o llama-server encerrou durante a inicialização ({reason}). Últimas linhas:\n{}",
                        self.logs.tail(10)
                    )));
                }
                _ if Instant::now() >= deadline => {
                    self.terminate(running).await;
                    return Err(EmbeddingError::Timeout(format!(
                        "o llama-server não ficou pronto em {} s. Últimas linhas:\n{}",
                        c.startup_timeout.as_secs(),
                        self.logs.tail(10)
                    )));
                }
                _ => tokio::time::sleep(POLL_INTERVAL).await,
            }
        }

        let record = PidRecord {
            pid,
            port,
            binary: c.binary.clone(),
            model_path: c.model_path.clone(),
        };
        if let Err(err) = pidfile::write(&c.pidfile(), &record) {
            tracing::warn!("could not write the llama-server pidfile: {err}");
        }
        tracing::info!(pid, port, "llama-server ready");
        Ok(running)
    }

    async fn terminate(&self, running: Running) {
        match running.child {
            Some(mut child) => {
                pidfile::signal(running.pid, "TERM");
                if tokio::time::timeout(self.config.stop_timeout, child.wait())
                    .await
                    .is_err()
                {
                    let _ = child.kill().await;
                }
            }
            None => self.kill_pid(running.pid).await,
        }
        self.logs.write(&format!("--- stopped pid {}", running.pid));
    }

    /// Stops a process we did not spawn in this session (only if it is our binary).
    async fn kill_pid(&self, pid: u32) {
        if !pidfile::is_our_server(pid, &self.config.binary) {
            return;
        }
        pidfile::signal(pid, "TERM");
        let deadline = Instant::now() + self.config.stop_timeout;
        while pidfile::is_our_server(pid, &self.config.binary) {
            if Instant::now() >= deadline {
                pidfile::signal(pid, "KILL");
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port())
}

fn random_key() -> std::io::Result<String> {
    let mut bytes = [0u8; 24];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Writes a file readable only by the user, created with 0600 (no window with wider permissions).
fn write_secret(path: &std::path::Path, secret: &str) -> std::io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(path);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(secret.as_bytes())?;
    // Guard against a pre-existing umask/ACL surprise.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}
