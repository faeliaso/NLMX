//! External model configuration (`embedding.json`), read at runtime.

use std::path::{Path, PathBuf};

use serde::Deserialize;

const POOLING: &[&str] = &["none", "mean", "cls", "last", "rank"];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingConfig {
    /// Stable identifier of the vector space, e.g. "qwen3-embedding-0.6b-q8_0".
    pub model_id: String,
    /// GGUF file; relative paths are resolved against the configuration file's directory.
    pub model_path: PathBuf,
    /// llama-server `--pooling` (`none`, `mean`, `cls`, `last`, `rank`).
    pub pooling: String,
    #[serde(default)]
    pub query_prefix: String,
    #[serde(default)]
    pub passage_prefix: String,
    #[serde(default = "defaults::context_size")]
    pub context_size: u32,
    #[serde(default = "defaults::batch_size")]
    pub batch_size: u32,
    /// Inputs per `/v1/embeddings` request.
    #[serde(default = "defaults::max_batch_inputs")]
    pub max_batch_inputs: usize,
    #[serde(default = "defaults::gpu_layers")]
    pub gpu_layers: u32,
}

mod defaults {
    pub fn context_size() -> u32 {
        8192
    }
    pub fn batch_size() -> u32 {
        2048
    }
    pub fn max_batch_inputs() -> usize {
        32
    }
    pub fn gpu_layers() -> u32 {
        99
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl EmbeddingConfig {
    /// Reads, resolves and validates a configuration file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|err| {
            ConfigError(format!("não foi possível ler {}: {err}", path.display()))
        })?;
        let mut config: Self = serde_json::from_str(&raw)
            .map_err(|err| ConfigError(format!("{} inválido: {err}", path.display())))?;
        if config.model_path.is_relative() {
            let base = path.parent().unwrap_or(Path::new("."));
            config.model_path = base.join(&config.model_path);
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.model_id.trim().is_empty() {
            return Err(ConfigError("model_id não pode ser vazio".into()));
        }
        if !self.model_path.is_file() {
            return Err(ConfigError(format!(
                "modelo não encontrado: {}",
                self.model_path.display()
            )));
        }
        if !POOLING.contains(&self.pooling.as_str()) {
            return Err(ConfigError(format!(
                "pooling \"{}\" inválido (use {})",
                self.pooling,
                POOLING.join(", ")
            )));
        }
        if self.context_size == 0 || self.batch_size == 0 || self.max_batch_inputs == 0 {
            return Err(ConfigError(
                "context_size, batch_size e max_batch_inputs devem ser > 0".into(),
            ));
        }
        Ok(())
    }
}
