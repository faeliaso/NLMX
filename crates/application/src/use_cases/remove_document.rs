//! Removes a document and everything derived from it: pages, chunks, lexical and vector index
//! entries, the library copy and the chat history that used it (see `RemovalImpact`). The file
//! the user imported from is never touched.

use std::{fmt, sync::Arc, time::Instant};

use nlmx_domain::{
    ingestion::{DocumentId, RemovalImpact},
    telemetry::Measurement,
};

use super::viewer::ViewDocument;
use crate::ports::{DocumentRepository, FileStore, StorageError};
use crate::telemetry::{ms, record};

pub struct RemoveDocument {
    pub documents: Arc<dyn DocumentRepository>,
    pub files: Arc<dyn FileStore>,
    /// Its per-page text cache is keyed by document id, and ids can be reused.
    pub viewer: Option<Arc<ViewDocument>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveError {
    NotFound,
    /// Still being read; the ingestion would race with the removal.
    Busy,
    Storage(String),
}

impl fmt::Display for RemoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("O documento não existe mais."),
            Self::Busy => f.write_str(
                "O documento ainda está sendo processado. Remova-o quando a leitura terminar.",
            ),
            Self::Storage(reason) => write!(f, "Não foi possível remover o documento: {reason}"),
        }
    }
}

impl From<StorageError> for RemoveError {
    fn from(err: StorageError) -> Self {
        Self::Storage(err.message)
    }
}

impl RemoveDocument {
    /// What `remove` would take with it, for the confirmation dialog.
    pub async fn impact(&self, id: DocumentId) -> Result<RemovalImpact, RemoveError> {
        self.check(id).await?;
        Ok(self.documents.removal_impact(id).await?)
    }

    pub async fn remove(&self, id: DocumentId) -> Result<RemovalImpact, RemoveError> {
        let started = Instant::now();
        self.check(id).await?;
        let removed = self
            .documents
            .remove(id)
            .await?
            .ok_or(RemoveError::NotFound)?;
        // The database is the source of truth: a library file that cannot be deleted now is
        // found by `prune_library` at the next start.
        if let Err(err) = self.files.remove(&removed.sha256).await {
            tracing::warn!(
                document_id = id,
                "library file not deleted: {}",
                err.message
            );
        }
        if let Some(viewer) = &self.viewer {
            viewer.forget(id);
        }
        let impact = removed.impact;
        record(&Measurement::DocumentRemoved {
            document_id: id,
            chunks: impact.chunks,
            turns: impact.turns,
            conversations: impact.conversations,
            total_ms: ms(started),
        });
        Ok(impact)
    }

    /// Deletes library files no document refers to (left by a removal whose file deletion
    /// failed, or by an interrupted import). Run at startup, before imports.
    pub async fn prune_library(&self) -> Result<u32, StorageError> {
        let keep = self.documents.hashes().await?;
        self.files.prune(keep).await
    }

    async fn check(&self, id: DocumentId) -> Result<(), RemoveError> {
        let record = self.documents.get(id).await?.ok_or(RemoveError::NotFound)?;
        if record.status.is_unfinished() {
            return Err(RemoveError::Busy);
        }
        Ok(())
    }
}
