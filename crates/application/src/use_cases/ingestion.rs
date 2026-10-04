//! The ingestion pipeline: PDF → metadata → pages → text → normalization → structure → chunks.
//!
//! Idempotent: a file already in the library (same SHA-256) is reported as a duplicate, and
//! re-running `ingest` for a document replaces its pages and chunks with an identical result.

use std::{path::Path, sync::Arc, time::Instant};

use nlmx_domain::{
    document::{DocumentError, DocumentHandle, DocumentMetadata},
    ingestion::{
        ChunkPolicy, DocumentId, DocumentStatus, DocumentSummary, ImportOutcome, PageLayout,
    },
    telemetry::{ErrorKind, IngestStage, Measurement},
};

use super::embeddings::{EmbedDocuments, EmbedOutcome};
use crate::ports::{
    Chunker, DocumentEngine, DocumentRepository, Extraction, FileStore, InsertOutcome, NewDocument,
    PageRecord, StorageError, StructureAnalyzer, TokenCounter,
};
use crate::telemetry::{ms, record};

pub struct DocumentIngestion {
    pub engine: Arc<dyn DocumentEngine>,
    pub files: Arc<dyn FileStore>,
    pub documents: Arc<dyn DocumentRepository>,
    pub analyzer: Arc<dyn StructureAnalyzer>,
    pub chunker: Arc<dyn Chunker>,
    pub tokens: Arc<dyn TokenCounter>,
    pub policy: ChunkPolicy,
    /// Embeds the chunks right after they are stored (`None`: documents wait for embeddings).
    pub embedder: Option<Arc<EmbedDocuments>>,
}

impl DocumentIngestion {
    /// Imports a file: duplicate check by content hash, copy into the library, then ingestion.
    /// A previously failed or interrupted import of the same file is retried.
    pub async fn import(&self, path: &Path) -> ImportOutcome {
        let failed = |reason: String| ImportOutcome::Failed { id: None, reason };
        let digest = match self.files.digest(path).await {
            Ok(digest) => digest,
            Err(err) => return failed(err.message),
        };

        let existing = match self.documents.find_by_sha256(&digest.sha256).await {
            Ok(existing) => existing,
            Err(err) => return failed(err.message),
        };
        if let Some(id) = existing {
            return self.retry(id).await;
        }

        let library_path = match self.files.store(path, &digest.sha256).await {
            Ok(library_path) => library_path,
            Err(err) => return failed(err.message),
        };
        let new = NewDocument {
            sha256: digest.sha256,
            original_filename: file_name(path),
            original_path: path.display().to_string(),
            library_path: library_path.display().to_string(),
            file_size: digest.size,
        };
        match self.documents.insert(new).await {
            Ok(InsertOutcome::Inserted(id)) => self.ingest(id).await,
            // Imported concurrently by someone else between the check and the insert.
            Ok(InsertOutcome::AlreadyExists(id)) => ImportOutcome::Duplicate { id },
            Err(err) => failed(err.message),
        }
    }

    /// Re-runs the pipeline of a failed or interrupted document; any other one is reported as a
    /// duplicate (there is nothing to redo).
    pub async fn retry(&self, id: DocumentId) -> ImportOutcome {
        match self.documents.get(id).await {
            Ok(Some(doc)) if doc.status == DocumentStatus::Failed || doc.status.is_unfinished() => {
                self.ingest(id).await
            }
            Ok(_) => ImportOutcome::Duplicate { id },
            Err(err) => ImportOutcome::Failed {
                id: Some(id),
                reason: err.message,
            },
        }
    }

    /// Runs (or re-runs) the pipeline for a stored document. Nothing derived from the file is
    /// written until the final atomic save, so an interruption leaves no partial chunks.
    pub async fn ingest(&self, id: DocumentId) -> ImportOutcome {
        let started = Instant::now();
        let result = self.run_pipeline(id).await;
        match &result {
            Ok(run) => record(&Measurement::Ingested {
                document_id: id,
                bytes: run.bytes,
                pages: run.pages,
                chunks: run.chunks,
                extract_ms: run.extract_ms,
                structure_ms: run.structure_ms,
                chunk_ms: run.chunk_ms,
                save_ms: run.save_ms,
                total_ms: ms(started),
            }),
            Err(f) => record(&Measurement::IngestFailed {
                document_id: id,
                stage: f.stage,
                kind: f.kind,
                total_ms: ms(started),
            }),
        }
        match result
            .map(|run| (run.chunks, run.status))
            .map_err(|f| f.reason)
        {
            Ok((chunks, DocumentStatus::Embedding)) => {
                let status = match &self.embedder {
                    Some(embedder) => match embedder.embed_document(id).await {
                        EmbedOutcome::Indexed { .. } => DocumentStatus::Indexed,
                        EmbedOutcome::NoModel | EmbedOutcome::Failed(_) => {
                            DocumentStatus::Embedding
                        }
                    },
                    None => DocumentStatus::Embedding,
                };
                ImportOutcome::Imported { id, chunks, status }
            }
            Ok((chunks, status)) => ImportOutcome::Imported { id, chunks, status },
            Err(reason) => {
                // Best effort: if even this fails, the document stays unfinished and is resumed later.
                let _ = self
                    .documents
                    .set_status(id, DocumentStatus::Failed, Some(reason.clone()))
                    .await;
                ImportOutcome::Failed {
                    id: Some(id),
                    reason,
                }
            }
        }
    }

    async fn run_pipeline(&self, id: DocumentId) -> Result<PipelineRun, Failure> {
        let storage = |stage| move |err: StorageError| Failure::storage(stage, err);
        let document = self
            .documents
            .get(id)
            .await
            .map_err(storage(IngestStage::Read))?
            .ok_or_else(|| Failure {
                reason: format!("documento {id} não encontrado"),
                stage: IngestStage::Read,
                kind: ErrorKind::NotFound,
            })?;

        let extract_started = Instant::now();
        self.documents
            .set_status(id, DocumentStatus::Extracting, None)
            .await
            .map_err(storage(IngestStage::Extract))?;
        let handle = self
            .engine
            .open(Path::new(&document.library_path))
            .await
            .map_err(Failure::document)?;
        let read = self.read(handle).await;
        // Always release the engine's document, even when reading failed.
        let _ = self.engine.close(handle).await;
        let (metadata, pages) = read.map_err(Failure::document)?;
        let extract_ms = ms(extract_started);

        let structure_started = Instant::now();
        self.documents
            .set_status(id, DocumentStatus::Structuring, None)
            .await
            .map_err(storage(IngestStage::Structure))?;
        let structured = self.analyzer.analyze(&pages);
        let structure_ms = ms(structure_started);

        let chunk_started = Instant::now();
        self.documents
            .set_status(id, DocumentStatus::Chunking, None)
            .await
            .map_err(storage(IngestStage::Chunk))?;
        let chunks = self
            .chunker
            .chunk(&structured, &self.policy, self.tokens.as_ref());
        let chunk_ms = ms(chunk_started);

        let has_text_layer = pages.iter().any(|p| p.has_text);
        let status = if has_text_layer {
            DocumentStatus::Embedding
        } else {
            DocumentStatus::NeedsOcr
        };
        let title = metadata
            .title
            .clone()
            .or_else(|| structured.first_heading().map(str::to_string))
            .unwrap_or_else(|| title_from_filename(&document.original_filename));
        let chunk_count = chunks.len() as u32;
        let page_count = pages.len() as u32;
        let extraction = Extraction {
            title,
            author: metadata.author,
            pdf_created_at: metadata.created_at,
            page_count: metadata.page_count,
            has_text_layer,
            pages: pages
                .iter()
                .map(|p| PageRecord {
                    number: p.number,
                    width: p.width,
                    height: p.height,
                    char_count: p.char_count,
                    has_text: p.has_text,
                })
                .collect(),
            chunks,
            extractor_version: self.analyzer.version(),
            chunker_version: self.chunker.version(),
            status,
        };
        let save_started = Instant::now();
        self.documents
            .save_extraction(id, extraction)
            .await
            .map_err(storage(IngestStage::Save))?;
        Ok(PipelineRun {
            chunks: chunk_count,
            status,
            bytes: document.file_size,
            pages: page_count,
            extract_ms,
            structure_ms,
            chunk_ms,
            save_ms: ms(save_started),
        })
    }

    async fn read(
        &self,
        handle: DocumentHandle,
    ) -> Result<(DocumentMetadata, Vec<PageLayout>), DocumentError> {
        let metadata = self.engine.metadata(handle).await?;
        let mut pages = Vec::with_capacity(metadata.page_count as usize);
        for number in 1..=metadata.page_count {
            let info = self.engine.page_info(handle, number).await?;
            let spans = if info.has_text {
                self.engine.text_spans(handle, number).await?
            } else {
                Vec::new()
            };
            pages.push(PageLayout {
                number,
                width: info.width,
                height: info.height,
                has_text: info.has_text,
                char_count: info.char_count,
                spans,
            });
        }
        Ok((metadata, pages))
    }

    /// Re-runs every document whose ingestion was interrupted (e.g. the app quit mid-import).
    pub async fn resume(&self) -> Vec<ImportOutcome> {
        let ids = match self.documents.unfinished().await {
            Ok(ids) => ids,
            Err(err) => {
                return vec![ImportOutcome::Failed {
                    id: None,
                    reason: err.message,
                }];
            }
        };
        let mut outcomes = Vec::with_capacity(ids.len());
        for id in ids {
            outcomes.push(self.ingest(id).await);
        }
        outcomes
    }

    pub async fn list(&self) -> Result<Vec<DocumentSummary>, StorageError> {
        self.documents.list().await
    }
}

/// What a successful pipeline run produced, with the time spent in each stage.
struct PipelineRun {
    chunks: u32,
    status: DocumentStatus,
    bytes: u64,
    pages: u32,
    extract_ms: u64,
    structure_ms: u64,
    chunk_ms: u64,
    save_ms: u64,
}

/// A failed run: the message for the user and, for measurements, where and why (no text).
struct Failure {
    reason: String,
    stage: IngestStage,
    kind: ErrorKind,
}

impl Failure {
    fn storage(stage: IngestStage, err: StorageError) -> Self {
        Self {
            reason: err.message,
            stage,
            kind: ErrorKind::Storage,
        }
    }

    fn document(err: DocumentError) -> Self {
        let kind = match &err {
            DocumentError::NotFound => ErrorKind::NotFound,
            DocumentError::InvalidPdf(_) => ErrorKind::Corrupt,
            DocumentError::PasswordRequired => ErrorKind::Refused,
            DocumentError::PageOutOfRange { .. } | DocumentError::UnknownHandle => {
                ErrorKind::Invalid
            }
            DocumentError::Engine(_) => ErrorKind::Other,
        };
        Self {
            reason: describe(err),
            stage: IngestStage::Extract,
            kind,
        }
    }
}

fn describe(err: DocumentError) -> String {
    let message = err.to_string();
    let mut chars = message.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or(message)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn title_from_filename(name: &str) -> String {
    let stem = name
        .strip_suffix(".pdf")
        .or_else(|| name.strip_suffix(".PDF"))
        .unwrap_or(name);
    stem.replace(['_', '-'], " ").trim().to_string()
}
