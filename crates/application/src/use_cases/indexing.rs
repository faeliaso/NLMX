//! The Indexação section: the state of the index and the background work, and the actions that
//! fix what went wrong (retry a failed document, embed what is pending, reindex everything).

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use nlmx_domain::{
    indexing::IndexSnapshot,
    ingestion::{DocumentId, DocumentStatus, ImportOutcome},
};

use super::{
    embeddings::{EmbedDocuments, EmbedOutcome},
    ingestion::DocumentIngestion,
};
use crate::ports::{DocumentRepository, EmbeddingSource, IndexingReader, StorageError};

/// Counts the indexing work running in the background (imports, startup resume, embeddings after
/// a model change, actions of the Indexação section).
#[derive(Debug, Default)]
pub struct IndexingActivity {
    running: AtomicUsize,
}

impl IndexingActivity {
    /// Registers work that may overlap with other work (imports, startup).
    pub fn begin(self: &Arc<Self>) -> ActivityGuard {
        self.running.fetch_add(1, Ordering::SeqCst);
        ActivityGuard(self.clone())
    }

    /// Registers work that must not overlap with any other (`None` while something runs).
    pub fn try_begin(self: &Arc<Self>) -> Option<ActivityGuard> {
        self.running
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| ActivityGuard(self.clone()))
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst) > 0
    }
}

/// Ends the registered work when dropped.
#[derive(Debug)]
pub struct ActivityGuard(Arc<IndexingActivity>);

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.0.running.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexingError {
    /// Other indexing work is running.
    Busy,
    Unavailable(String),
}

impl fmt::Display for IndexingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str(
                "A indexação já está em andamento. Tente novamente quando ela terminar.",
            ),
            Self::Unavailable(reason) => f.write_str(reason),
        }
    }
}

/// What the section shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexingReport {
    pub snapshot: IndexSnapshot,
    /// The configured embedding model (`None`: search is lexical only).
    pub model_id: Option<String>,
    /// Background work is running.
    pub running: bool,
}

impl IndexingReport {
    /// Something will change without the user doing anything (the page refreshes itself).
    pub fn active(&self) -> bool {
        self.running || self.snapshot.reading() > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryOutcome {
    /// A failed document was read again.
    Ingested(ImportOutcome),
    /// A document waiting for embeddings was embedded again.
    Embedded(EmbedOutcome),
    /// Neither failed nor waiting.
    NothingToDo,
}

pub struct Indexing {
    pub reader: Arc<dyn IndexingReader>,
    pub documents: Arc<dyn DocumentRepository>,
    pub embeddings: Arc<dyn EmbeddingSource>,
    pub embedder: Arc<EmbedDocuments>,
    /// Unavailable (with the reason) without the PDF engine: failed documents cannot be re-read.
    pub ingestion: Result<Arc<DocumentIngestion>, String>,
    pub activity: Arc<IndexingActivity>,
}

impl Indexing {
    pub async fn report(&self) -> Result<IndexingReport, StorageError> {
        let model_id = self.embeddings.current().map(|p| p.model_id().to_string());
        Ok(IndexingReport {
            snapshot: self.reader.snapshot(model_id.clone()).await?,
            model_id,
            running: self.activity.is_running(),
        })
    }

    /// Claims the index for one action; refused while other indexing work runs.
    pub fn begin(&self) -> Result<ActivityGuard, IndexingError> {
        self.activity.try_begin().ok_or(IndexingError::Busy)
    }

    /// Reads a failed document again, or embeds again one whose embeddings failed.
    pub async fn retry(&self, id: DocumentId) -> Result<RetryOutcome, IndexingError> {
        let status = self
            .documents
            .get(id)
            .await
            .map_err(|e| IndexingError::Unavailable(e.message))?
            .map(|d| d.status)
            .ok_or_else(|| IndexingError::Unavailable("O documento não existe mais.".into()))?;
        match status {
            DocumentStatus::Failed => {
                let ingestion = self
                    .ingestion
                    .as_ref()
                    .map_err(|reason| IndexingError::Unavailable(reason.clone()))?;
                Ok(RetryOutcome::Ingested(ingestion.retry(id).await))
            }
            DocumentStatus::Embedding => Ok(RetryOutcome::Embedded(
                self.embedder.embed_document(id).await,
            )),
            _ => Ok(RetryOutcome::NothingToDo),
        }
    }

    /// Retries every failed document.
    pub async fn retry_failed(&self) -> Result<Vec<RetryOutcome>, IndexingError> {
        let failed: Vec<DocumentId> = self
            .documents
            .list()
            .await
            .map_err(|e| IndexingError::Unavailable(e.message))?
            .into_iter()
            .filter(|d| d.status == DocumentStatus::Failed)
            .map(|d| d.id)
            .collect();
        let mut outcomes = Vec::with_capacity(failed.len());
        for id in failed {
            outcomes.push(self.retry(id).await?);
        }
        Ok(outcomes)
    }

    pub async fn embed_pending(&self) -> Vec<(DocumentId, EmbedOutcome)> {
        self.embedder.embed_pending().await
    }

    /// Embeds every indexed document again with the current model.
    pub async fn reindex_all(&self) -> Result<Vec<(DocumentId, EmbedOutcome)>, IndexingError> {
        if self.embeddings.current().is_none() {
            return Err(IndexingError::Unavailable(
                "Nenhum modelo de embeddings ativo. Ative um modelo em Modelos.".into(),
            ));
        }
        Ok(self.embedder.reindex_all().await)
    }
}
