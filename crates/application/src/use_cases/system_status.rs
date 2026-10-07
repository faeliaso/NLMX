use std::sync::Arc;

use nlmx_domain::{generation::LanguageModelStatus, models::RuntimeInfo};

use crate::ports::{InferenceRuntime, LlmProvider, StorageDiagnostics, StorageError, StorageInfo};

/// Snapshot of the runtime dependencies shown in the app's status area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemStatus {
    pub language_model: LanguageModelStatus,
    pub storage: Result<StorageInfo, StorageError>,
    /// The PDF engine build when it loaded, or why it did not.
    pub document_engine: Result<String, String>,
    /// The bundled inference runtime (llama.cpp).
    pub runtime: Result<RuntimeInfo, String>,
}

pub struct GetSystemStatus {
    language_model: Arc<dyn LlmProvider>,
    storage: Arc<dyn StorageDiagnostics>,
    document_engine: Result<String, String>,
    runtime: Arc<dyn InferenceRuntime>,
}

impl GetSystemStatus {
    /// `document_engine` is fixed at startup: the PDF library either loaded or it did not.
    pub fn new(
        language_model: Arc<dyn LlmProvider>,
        storage: Arc<dyn StorageDiagnostics>,
        document_engine: Result<String, String>,
        runtime: Arc<dyn InferenceRuntime>,
    ) -> Self {
        Self {
            language_model,
            storage,
            document_engine,
            runtime,
        }
    }

    /// The language model's status, queried afresh (never from a cache).
    pub async fn recheck_language_model(&self) -> LanguageModelStatus {
        self.language_model.recheck().await
    }

    /// The language model's status as the provider reports it now.
    pub async fn language_model(&self) -> LanguageModelStatus {
        self.language_model.status().await
    }

    pub async fn execute(&self) -> SystemStatus {
        SystemStatus {
            language_model: self.language_model.status().await,
            storage: self.storage.info().await,
            document_engine: self.document_engine.clone(),
            runtime: self.runtime.info().await,
        }
    }
}
