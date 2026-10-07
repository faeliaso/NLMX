//! `ModelProvider`: the embedded catalog, verified on-demand downloads into Application Support,
//! integrity checks, updates, removal, disk-space control and the active embedding model.
//!
//! Models are separate from the runtime (llama.cpp is bundled with the app). Nothing is downloaded
//! without a `ConfirmedDownload`, which only the user's confirmation of a `DownloadPlan` produces.

mod catalog;
mod disk;
mod download;
mod store;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use nlmx_application::ports::{BoxFuture, CancelFlag, ModelProvider, ProgressCallback};
use nlmx_domain::models::{
    ConfirmedDownload, DiskSpace, DownloadPlan, InstalledModel, ModelDescriptor, ModelError,
    ModelState, VerifyReport,
};
use serde::{Deserialize, Serialize};

pub use catalog::{builtin as builtin_catalog, parse as parse_catalog};
pub use disk::DiskMeter;
use store::{Manifest, Store, io, now_iso, quick_check};

/// Directory (inside the app data directory) that holds downloaded models.
pub const MODELS_DIR: &str = "models";

pub struct LocalModelProvider {
    store: Store,
    catalog: Vec<ModelDescriptor>,
    /// `embedding.json` consumed by the embedding provider; written by `activate`.
    embedding_config: PathBuf,
    client: reqwest::Client,
    disk: DiskMeter,
    downloading: Mutex<HashSet<String>>,
}

/// Same keys as `nlmx_embed_llama::EmbeddingConfig` (checked by the workspace tests).
#[derive(Serialize, Deserialize)]
struct EmbeddingConfigFile {
    model_id: String,
    model_path: PathBuf,
    pooling: String,
    query_prefix: String,
    passage_prefix: String,
    context_size: u32,
    batch_size: u32,
    max_batch_inputs: usize,
    gpu_layers: u32,
}

impl LocalModelProvider {
    pub fn new(
        models_root: PathBuf,
        embedding_config: PathBuf,
        catalog: Vec<ModelDescriptor>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(30))
            .user_agent(concat!("NLMX/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("HTTP client");
        Self {
            store: Store::new(models_root),
            catalog,
            embedding_config,
            client,
            disk: disk::system(),
            downloading: Mutex::new(HashSet::new()),
        }
    }

    /// Uses `<data_dir>/models` and `<data_dir>/embedding.json` with the embedded catalog.
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self::new(
            data_dir.join(MODELS_DIR),
            data_dir.join("embedding.json"),
            builtin_catalog(),
        )
    }

    pub fn with_disk_meter(mut self, meter: DiskMeter) -> Self {
        self.disk = meter;
        self
    }

    fn descriptor(&self, id: &str) -> Result<&ModelDescriptor, ModelError> {
        self.catalog
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| ModelError::UnknownModel(id.to_string()))
    }

    /// The newest installed version that passes the quick check.
    fn installed_version(&self, id: &str) -> Option<(Manifest, PathBuf)> {
        self.store
            .manifests(id)
            .into_iter()
            .find(|(m, dir)| quick_check(m, dir).is_ok())
    }

    fn available(&self) -> Result<u64, ModelError> {
        (self.disk)(self.store.root()).map_err(io)
    }

    fn plan(
        &self,
        model: &ModelDescriptor,
        replaces: Option<String>,
    ) -> Result<DownloadPlan, ModelError> {
        let resume_from = std::fs::metadata(self.store.part_path(model))
            .map(|m| m.len().min(model.size))
            .unwrap_or(0);
        Ok(DownloadPlan {
            model: model.clone(),
            resume_from,
            required_bytes: model.size - resume_from + disk::margin(model.size),
            available_bytes: self.available()?,
            replaces,
        })
    }

    fn active_id(&self) -> Option<String> {
        let raw = std::fs::read_to_string(&self.embedding_config).ok()?;
        serde_json::from_str::<EmbeddingConfigFile>(&raw)
            .ok()
            .map(|c| c.model_id)
    }

    fn write_embedding_config(
        &self,
        model: &ModelDescriptor,
        path: &str,
    ) -> Result<(), ModelError> {
        let config = EmbeddingConfigFile {
            model_id: model.id.clone(),
            model_path: PathBuf::from(path),
            pooling: model.pooling.clone(),
            query_prefix: model.query_prefix.clone(),
            passage_prefix: model.passage_prefix.clone(),
            context_size: model.context_size,
            batch_size: 2048,
            max_batch_inputs: 32,
            gpu_layers: 99,
        };
        if let Some(dir) = self.embedding_config.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let tmp = self.embedding_config.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&config).expect("serializable"),
        )
        .map_err(io)?;
        std::fs::rename(&tmp, &self.embedding_config).map_err(io)
    }

    async fn run_download(
        &self,
        confirmed: ConfirmedDownload,
        progress: ProgressCallback,
        cancel: CancelFlag,
    ) -> Result<InstalledModel, ModelError> {
        let plan = confirmed.plan();
        let model = self.descriptor(&plan.model.id)?.clone();
        if model.version != plan.model.version || model.sha256 != plan.model.sha256 {
            return Err(ModelError::Io(
                "o catálogo mudou desde a confirmação; confirme novamente".into(),
            ));
        }
        // Space may have changed since the plan was shown.
        let fresh = self.plan(&model, plan.replaces.clone())?;
        if !fresh.fits_on_disk() {
            return Err(ModelError::InsufficientSpace {
                required: fresh.required_bytes,
                available: fresh.available_bytes,
            });
        }

        let part = self.store.part_path(&model);
        let sha256 =
            download::fetch(&self.client, &model, &part, &self.disk, &progress, &cancel).await?;
        let dir = self.store.version_dir(&model.id, &model.version);
        let file = self.store.file_path(&model);
        tokio::fs::rename(&part, &file).await.map_err(io)?;
        let manifest = Manifest {
            id: model.id.clone(),
            version: model.version.clone(),
            file: model.file_name.clone(),
            size: model.size,
            sha256,
            source_url: model.url.clone(),
            installed_at: now_iso(),
            verified_at: Some(now_iso()),
        };
        self.store.write_manifest(&dir, &manifest)?;
        let installed = Store::installed(&manifest, &dir);
        tracing::info!(model = %model.id, version = %model.version, "model installed");

        // Update: switch the active model to the new file, then drop the other versions.
        if self.active_id().as_deref() == Some(model.id.as_str()) {
            self.write_embedding_config(&model, &installed.path)?;
        }
        for (old, old_dir) in self.store.manifests(&model.id) {
            if old.version != model.version {
                let _ = std::fs::remove_dir_all(old_dir);
            }
        }
        Ok(installed)
    }
}

/// Releases the per-model download lock however the download ends.
struct DownloadLock<'a> {
    set: &'a Mutex<HashSet<String>>,
    id: String,
}

impl Drop for DownloadLock<'_> {
    fn drop(&mut self) {
        self.set.lock().unwrap().remove(&self.id);
    }
}

impl ModelProvider for LocalModelProvider {
    fn catalog(&self) -> Vec<ModelDescriptor> {
        self.catalog.clone()
    }

    fn installed(&self) -> BoxFuture<'_, Result<Vec<InstalledModel>, ModelError>> {
        Box::pin(async move {
            Ok(self
                .catalog
                .iter()
                .filter_map(|m| self.installed_version(&m.id))
                .map(|(manifest, dir)| Store::installed(&manifest, &dir))
                .collect())
        })
    }

    fn status<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ModelState, ModelError>> {
        Box::pin(async move {
            let model = self.descriptor(id)?;
            let manifests = self.store.manifests(id);
            let Some((manifest, dir)) = manifests.first() else {
                let part = std::fs::metadata(self.store.part_path(model))
                    .map(|m| m.len())
                    .unwrap_or(0);
                return Ok(if part > 0 {
                    ModelState::PartiallyDownloaded {
                        bytes: part,
                        total: model.size,
                    }
                } else {
                    ModelState::NotInstalled
                });
            };
            if let Err(reason) = quick_check(manifest, dir) {
                return Ok(ModelState::Corrupted { reason });
            }
            Ok(if manifest.version == model.version {
                ModelState::Installed {
                    version: manifest.version.clone(),
                }
            } else {
                ModelState::UpdateAvailable {
                    installed: manifest.version.clone(),
                    latest: model.version.clone(),
                }
            })
        })
    }

    fn plan_download<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<DownloadPlan, ModelError>> {
        Box::pin(async move {
            let model = self.descriptor(id)?;
            let installed = self.installed_version(id);
            if installed
                .as_ref()
                .is_some_and(|(m, _)| m.version == model.version)
            {
                return Err(ModelError::AlreadyInstalled(id.to_string()));
            }
            self.plan(model, installed.map(|(m, _)| m.version))
        })
    }

    fn download(
        &self,
        confirmed: ConfirmedDownload,
        progress: ProgressCallback,
        cancel: CancelFlag,
    ) -> BoxFuture<'_, Result<InstalledModel, ModelError>> {
        Box::pin(async move {
            let id = confirmed.plan().model.id.clone();
            if !self.downloading.lock().unwrap().insert(id.clone()) {
                return Err(ModelError::Io(format!(
                    "o modelo {id} já está sendo baixado"
                )));
            }
            let _lock = DownloadLock {
                set: &self.downloading,
                id,
            };
            self.run_download(confirmed, progress, cancel).await
        })
    }

    fn verify<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<VerifyReport, ModelError>> {
        Box::pin(async move {
            self.descriptor(id)?;
            let (mut manifest, dir) = self
                .store
                .manifests(id)
                .into_iter()
                .next()
                .ok_or_else(|| ModelError::NotInstalled(id.to_string()))?;
            let actual = download::sha256_file(&dir.join(&manifest.file)).await?;
            let ok = actual == manifest.sha256;
            if ok {
                manifest.verified_at = Some(now_iso());
                self.store.write_manifest(&dir, &manifest)?;
            } else {
                tracing::warn!(model = id, "model checksum mismatch: {actual}");
            }
            Ok(VerifyReport {
                model: Store::installed(&manifest, &dir),
                ok,
                actual_sha256: actual,
            })
        })
    }

    fn remove<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), ModelError>> {
        Box::pin(async move {
            self.descriptor(id)?;
            if self.downloading.lock().unwrap().contains(id) {
                return Err(ModelError::Io(format!(
                    "cancele o download de {id} antes de removê-lo"
                )));
            }
            if self.active_id().as_deref() == Some(id) {
                let _ = std::fs::remove_file(&self.embedding_config);
            }
            match std::fs::remove_dir_all(self.store.model_dir(id)) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(io(err)),
                _ => Ok(()),
            }
        })
    }

    fn plan_update<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<DownloadPlan, ModelError>> {
        Box::pin(async move {
            let model = self.descriptor(id)?;
            let (installed, _) = self
                .installed_version(id)
                .ok_or_else(|| ModelError::NotInstalled(id.to_string()))?;
            if installed.version == model.version {
                return Err(ModelError::AlreadyInstalled(id.to_string()));
            }
            self.plan(model, Some(installed.version))
        })
    }

    fn activate<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<InstalledModel, ModelError>> {
        Box::pin(async move {
            let model = self.descriptor(id)?;
            let (manifest, dir) = self
                .installed_version(id)
                .ok_or_else(|| ModelError::NotInstalled(id.to_string()))?;
            let installed = Store::installed(&manifest, &dir);
            self.write_embedding_config(model, &installed.path)?;
            Ok(installed)
        })
    }

    fn active(&self) -> BoxFuture<'_, Result<Option<InstalledModel>, ModelError>> {
        Box::pin(async move {
            Ok(self
                .active_id()
                .and_then(|id| self.installed_version(&id))
                .map(|(manifest, dir)| Store::installed(&manifest, &dir)))
        })
    }

    fn disk(&self) -> BoxFuture<'_, Result<DiskSpace, ModelError>> {
        Box::pin(async move {
            Ok(DiskSpace {
                models_bytes: self.store.usage(),
                available_bytes: self.available()?,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalog's URLs are https://: the client must speak TLS. Without a TLS backend reqwest
    /// rejects the URL before connecting (a builder error) — which once broke every download.
    #[tokio::test]
    async fn the_http_client_speaks_https() {
        let dir = std::env::temp_dir().join(format!("nlmx-models-tls-{}", std::process::id()));
        let provider =
            LocalModelProvider::new(dir.join("models"), dir.join("embedding.json"), Vec::new());
        // Nothing listens on port 9: a TLS-capable client fails to *connect*.
        let err = provider
            .client
            .get("https://127.0.0.1:9/model.gguf")
            .send()
            .await
            .unwrap_err();
        assert!(err.is_connect(), "{err:?}");
    }

    #[test]
    fn every_catalog_url_is_https() {
        for model in catalog::builtin() {
            assert!(model.url.starts_with("https://"), "{}", model.url);
        }
    }
}
