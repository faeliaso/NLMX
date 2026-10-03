//! `EmbeddingProvider` backed by llama.cpp's `llama-server`, run as a child process supervised by
//! the app and bound to 127.0.0.1 with a per-run API key (ADR 0006).

mod config;
mod logs;
mod pidfile;
mod provider;
mod runtime;
mod server;

use std::path::{Path, PathBuf};

pub use config::{ConfigError, EmbeddingConfig};
pub use provider::LlamaCppEmbeddingProvider;
pub use runtime::LlamaCppRuntime;
pub use server::{Endpoint, Health, LlamaServer, LlamaServerConfig};

/// llama.cpp build fetched by scripts/bootstrap.sh.
pub const LLAMA_BUILD: &str = "b11349";

/// Where to find `llama-server`, in order: `NLMX_LLAMA_SERVER`; next to the app executable
/// (bundled); `runtime/llama/` of the workspace (debug builds only).
pub fn llama_server_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("NLMX_LLAMA_SERVER") {
        return Some(PathBuf::from(path));
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    if let Some(dir) = exe_dir {
        // The Tauri sidecar: `Contents/MacOS/llama-server`, libraries in `Contents/Frameworks`.
        let sidecar = dir.join("llama-server");
        if sidecar.is_file() {
            return Some(sidecar);
        }
    }
    if cfg!(debug_assertions) {
        let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../runtime/llama/llama-server");
        if dev.is_file() {
            return Some(dev);
        }
    }
    None
}

/// The embedding configuration file: `NLMX_EMBEDDING_CONFIG`, else `<data_dir>/embedding.json`.
pub fn config_path(data_dir: &Path) -> PathBuf {
    std::env::var_os("NLMX_EMBEDDING_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("embedding.json"))
}

/// Builds the server supervisor and provider from a validated configuration.
pub fn provider(
    config: EmbeddingConfig,
    binary: PathBuf,
    data_dir: &Path,
) -> LlamaCppEmbeddingProvider {
    let mut server = LlamaServerConfig::new(binary, config.model_path.clone(), data_dir);
    server.pooling = config.pooling.clone();
    server.context_size = config.context_size;
    server.batch_size = config.batch_size;
    server.gpu_layers = config.gpu_layers;
    LlamaCppEmbeddingProvider::new(config, std::sync::Arc::new(LlamaServer::new(server)))
}
