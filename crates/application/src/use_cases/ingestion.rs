//! The ingestion pipeline of a document of any supported format: file → parser (picked by the
//! `ParserRegistry`) → normalization → structure → chunks → (embeddings) → indexed. Without a
//! `ContentPipeline` it falls back to the legacy PDF-only path (PDF → pages → structure → chunks).
//!
//! Idempotent: a file already in the library (same SHA-256) is reported as a duplicate, and
//! re-running `ingest` for a document replaces its pages and chunks with an identical result.

use std::{fmt::Display, path::Path, sync::Arc, time::Instant};

use nlmx_domain::{
    document::{DocumentError, DocumentHandle, DocumentMetadata},
    document_type::DocumentType,
    ingestion::{
        ChunkPolicy, DocumentId, DocumentStatus, DocumentSummary, ImportOutcome, IngestPhase,
        IngestProgress, PageLayout,
    },
    note::{clean_note_text, note_title},
    parsed::ParseError,
    telemetry::{ErrorKind, IngestStage, Measurement},
};

use super::{
    embeddings::{EMBED_START, EmbedDocuments, EmbedOutcome},
    viewer::ViewDocument,
};
use crate::ports::{
    BoxFuture, Chunker, DocumentEngine, DocumentRecord, DocumentRepository, DocumentSource,
    Extraction, FileDigest, FileStore, InsertOutcome, NewDocument, PageRecord, ProgressSink,
    StorageError, StructureAnalyzer, TokenCounter,
};
use crate::services::{
    parsing::read_layouts,
    pipeline::{ContentPipeline, PipelineStage, StageObserver},
};
use crate::telemetry::{ms, record};

/// What registering a file produced (see [`DocumentIngestion::enqueue`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enqueued {
    /// A new document, waiting in status `queued`.
    New(DocumentId),
    /// The file of a known document changed: it keeps its id and waits to be indexed again.
    Updated(DocumentId),
    /// The same file failed or was interrupted before: it is processed again.
    Retry(DocumentId),
    /// Already in the library, nothing to do.
    Duplicate(DocumentId),
    Failed {
        id: Option<DocumentId>,
        reason: String,
    },
}

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
    /// The multi-format content pipeline. `None`: the legacy path, which only reads PDFs.
    pub pipeline: Option<Arc<ContentPipeline>>,
    /// Told what the pipeline is doing (`None`: nobody listens).
    pub progress: Option<Arc<dyn ProgressSink>>,
    /// Drops its cached text of a document whose file changed.
    pub viewer: Option<Arc<ViewDocument>>,
}

impl DocumentIngestion {
    /// Imports a file: [`enqueue`](Self::enqueue) it, then ingest it. A previously failed or
    /// interrupted import of the same file is retried.
    pub async fn import(&self, path: &Path) -> ImportOutcome {
        let enqueued = self.enqueue(path).await;
        self.process(enqueued).await
    }

    /// Registers a file without processing it: duplicate check by content hash, copy into the
    /// library, then the document exists in status `queued` (visible in the library) and waits
    /// for [`process`](Self::process). Nothing derived from the file is written yet.
    pub async fn enqueue(&self, path: &Path) -> Enqueued {
        let failed = |reason: String| Enqueued::Failed { id: None, reason };
        let digest = match self.files.digest(path).await {
            Ok(digest) => digest,
            Err(err) => return failed(err.message),
        };

        let existing = match self.documents.find_by_sha256(&digest.sha256).await {
            Ok(existing) => existing,
            Err(err) => return failed(err.message),
        };
        if let Some(id) = existing {
            return match self.documents.get(id).await {
                // Failed: do it again. A document still being read is already in the queue (an
                // interrupted one is picked up at startup by `resume`): not a second run.
                Ok(Some(doc)) if doc.status == DocumentStatus::Failed => Enqueued::Retry(id),
                Ok(_) => Enqueued::Duplicate(id),
                Err(err) => Enqueued::Failed {
                    id: Some(id),
                    reason: err.message,
                },
            };
        }

        // Without parsers the only format is PDF (legacy path); with them, the registry decides.
        let document_type = match &self.pipeline {
            None => DocumentType::Pdf,
            Some(pipeline) => match DocumentType::from_path(path)
                .filter(|kind| pipeline.parsers.parser_for(*kind).is_some())
            {
                Some(kind) => kind,
                None => return failed(ParseError::Unsupported.to_string()),
            },
        };

        let original_path = path.display().to_string();
        if self.pipeline.is_some() {
            match self.documents.find_by_original_path(&original_path).await {
                Ok(Some(id)) => {
                    return self
                        .update_changed(id, path, digest, document_type, original_path)
                        .await;
                }
                Ok(None) => {}
                Err(err) => return failed(err.message),
            }
        }

        let library_path = match self.files.store(path, &digest.sha256).await {
            Ok(library_path) => library_path,
            Err(err) => return failed(err.message),
        };
        let new = NewDocument {
            sha256: digest.sha256,
            original_filename: file_name(path),
            original_path,
            library_path: library_path.display().to_string(),
            file_size: digest.size,
            document_type,
            note_text: None,
        };
        match self.documents.insert(new).await {
            Ok(InsertOutcome::Inserted(id)) => Enqueued::New(id),
            // Imported concurrently by someone else between the check and the insert.
            Ok(InsertOutcome::AlreadyExists(id)) => Enqueued::Duplicate(id),
            Err(err) => failed(err.message),
        }
    }

    /// Registers a note: pasted text, kept in the database (no file anywhere). The text is
    /// cleaned (`clean_note_text`), the same text is a duplicate, and the document waits in
    /// status `queued` for [`process`](Self::process) like any other source. Its title is
    /// stored as the document's name.
    pub async fn enqueue_note(&self, text: &str) -> Enqueued {
        let failed = |reason: String| Enqueued::Failed { id: None, reason };
        let text = match clean_note_text(text) {
            Ok(text) => text,
            Err(err) => return failed(err.to_string()),
        };
        let supported = self
            .pipeline
            .as_ref()
            .is_some_and(|pipeline| pipeline.parsers.parser_for(DocumentType::Note).is_some());
        if !supported {
            return failed(ParseError::Unsupported.to_string());
        }
        let digest = self.files.digest_text(&text);
        match self.documents.find_by_sha256(&digest.sha256).await {
            Ok(Some(id)) => {
                return match self.documents.get(id).await {
                    Ok(Some(doc)) if doc.status == DocumentStatus::Failed => Enqueued::Retry(id),
                    Ok(_) => Enqueued::Duplicate(id),
                    Err(err) => Enqueued::Failed {
                        id: Some(id),
                        reason: err.message,
                    },
                };
            }
            Ok(None) => {}
            Err(err) => return failed(err.message),
        }
        let new = NewDocument {
            sha256: digest.sha256,
            original_filename: note_title(&text),
            original_path: String::new(),
            library_path: String::new(),
            file_size: digest.size,
            document_type: DocumentType::Note,
            note_text: Some(text),
        };
        match self.documents.insert(new).await {
            Ok(InsertOutcome::Inserted(id)) => Enqueued::New(id),
            Ok(InsertOutcome::AlreadyExists(id)) => Enqueued::Duplicate(id),
            Err(err) => failed(err.message),
        }
    }

    /// Runs the pipeline for what [`enqueue`](Self::enqueue) registered.
    pub async fn process(&self, enqueued: Enqueued) -> ImportOutcome {
        match enqueued {
            Enqueued::New(id) | Enqueued::Updated(id) | Enqueued::Retry(id) => {
                self.ingest(id).await
            }
            Enqueued::Duplicate(id) => ImportOutcome::Duplicate { id },
            Enqueued::Failed { id, reason } => ImportOutcome::Failed { id, reason },
        }
    }

    /// The file of an imported document was changed (same original path, new hash): the document
    /// keeps its id, points at the new copy and is indexed again, replacing its chunks, lexical
    /// entries and vectors instead of adding to them.
    async fn update_changed(
        &self,
        id: DocumentId,
        path: &Path,
        digest: FileDigest,
        document_type: DocumentType,
        original_path: String,
    ) -> Enqueued {
        let failed = |reason: String| Enqueued::Failed {
            id: Some(id),
            reason,
        };
        let old = match self.documents.get(id).await {
            Ok(Some(old)) => old,
            Ok(None) => return failed(format!("documento {id} não encontrado")),
            Err(err) => return failed(err.message),
        };
        let library_path = match self.files.store(path, &digest.sha256).await {
            Ok(library_path) => library_path,
            Err(err) => return failed(err.message),
        };
        let new_sha = digest.sha256.clone();
        let new = NewDocument {
            sha256: digest.sha256,
            original_filename: file_name(path),
            original_path,
            library_path: library_path.display().to_string(),
            file_size: digest.size,
            document_type,
            note_text: None,
        };
        if let Err(err) = self.documents.replace_source(id, new).await {
            // Nobody references the copy just made.
            let _ = self.files.remove(&new_sha).await;
            return failed(err.message);
        }
        // The previous version's copy is no longer referenced by any document.
        let _ = self.files.remove(&old.sha256).await;
        if let Some(viewer) = &self.viewer {
            viewer.forget(id);
        }
        Enqueued::Updated(id)
    }

    /// Runs the whole pipeline again for a document, whatever its status. The previous chunks,
    /// lexical entries and vectors are replaced, never added to.
    pub async fn reindex(&self, id: DocumentId) -> ImportOutcome {
        self.ingest(id).await
    }

    /// The formats the app can import: the ones with a parser, or just PDF on the legacy path.
    pub fn supported_types(&self) -> Vec<DocumentType> {
        match &self.pipeline {
            Some(pipeline) => pipeline.parsers.supported_types(),
            None => vec![DocumentType::Pdf],
        }
    }

    /// Imports each file in turn. A file that fails does not stop the others: its outcome is
    /// `Failed` and the next one goes on.
    pub async fn import_many(
        &self,
        paths: &[std::path::PathBuf],
    ) -> Vec<(std::path::PathBuf, ImportOutcome)> {
        let mut outcomes = Vec::with_capacity(paths.len());
        for path in paths {
            outcomes.push((path.clone(), self.import(path).await));
        }
        outcomes
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
        let result = match &self.pipeline {
            Some(pipeline) => self.run_content_pipeline(pipeline, id).await,
            None => self.run_pipeline(id).await,
        };
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
                if status == DocumentStatus::Embedding {
                    self.report_waiting(id, chunks).await;
                }
                ImportOutcome::Imported { id, chunks, status }
            }
            Ok((chunks, status)) => ImportOutcome::Imported { id, chunks, status },
            Err(reason) => {
                // Best effort: if even this fails, the document stays unfinished and is resumed later.
                let _ = self
                    .documents
                    .set_status(id, DocumentStatus::Failed, Some(reason.clone()))
                    .await;
                self.report_failure(id).await;
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

    /// The multi-format path: parse → normalize → chunk (the `ContentPipeline`) → save. The
    /// status follows each stage; nothing derived from the file is written before the final
    /// atomic save, so an interruption leaves no partial chunks.
    async fn run_content_pipeline(
        &self,
        pipeline: &ContentPipeline,
        id: DocumentId,
    ) -> Result<PipelineRun, Failure> {
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

        self.documents
            .set_status(id, DocumentStatus::Extracting, None)
            .await
            .map_err(storage(IngestStage::Extract))?;
        let observer = Observer::new(self, &document);
        let source = match &document.note_text {
            Some(text) => DocumentSource::note(text.as_str()),
            None => DocumentSource::of_type(&document.library_path, document.document_type),
        };
        let processed = pipeline
            .run_with(
                &source,
                id,
                Some(document.original_filename.as_str()),
                &observer,
            )
            .await
            .map_err(Failure::parse)?;
        let (extract_ms, structure_ms, chunk_ms) = observer.timings();

        let page_count = processed.pages.len() as u32;
        let title = processed
            .metadata
            .title
            .clone()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| {
                // A note's name is already its title, not a file name to clean up.
                if document.document_type.is_note() {
                    document.original_filename.clone()
                } else {
                    title_from_filename(&document.original_filename)
                }
            });
        let stored = processed.into_stored(title);
        let (chunk_count, status) = (stored.chunks.len() as u32, stored.status);

        let save_started = Instant::now();
        self.documents
            .save_processed(id, stored)
            .await
            .map_err(storage(IngestStage::Save))?;
        self.report(
            &document,
            IngestPhase::Saving,
            SAVED,
            Some(chunk_count),
            status,
        );
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

    fn report(
        &self,
        document: &DocumentRecord,
        phase: IngestPhase,
        fraction: f32,
        chunks: Option<u32>,
        status: DocumentStatus,
    ) {
        if let Some(sink) = &self.progress {
            sink.report(IngestProgress::new(
                document.id,
                document.original_filename.clone(),
                document.document_type,
                phase,
                fraction,
                chunks,
                status,
            ));
        }
    }

    /// The chunks are stored but the embeddings have to wait (no model, or it failed).
    async fn report_waiting(&self, id: DocumentId, chunks: u32) {
        if self.progress.is_none() {
            return;
        }
        if let Ok(Some(document)) = self.documents.get(id).await {
            self.report(
                &document,
                IngestPhase::Waiting,
                SAVED,
                Some(chunks),
                DocumentStatus::Embedding,
            );
        }
    }

    async fn report_failure(&self, id: DocumentId) {
        if self.progress.is_none() {
            return;
        }
        if let Ok(Some(document)) = self.documents.get(id).await {
            self.report(
                &document,
                IngestPhase::Failed,
                0.0,
                None,
                DocumentStatus::Failed,
            );
        }
    }

    async fn read(
        &self,
        handle: DocumentHandle,
    ) -> Result<(DocumentMetadata, Vec<PageLayout>), DocumentError> {
        read_layouts(self.engine.as_ref(), handle).await
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

    fn parse(err: ParseError) -> Self {
        let kind = match &err {
            ParseError::NotFound => ErrorKind::NotFound,
            ParseError::Invalid(_) | ParseError::Encoding => ErrorKind::Corrupt,
            ParseError::Encrypted | ParseError::Drm => ErrorKind::Refused,
            ParseError::Unsupported | ParseError::Empty | ParseError::TooLarge => {
                ErrorKind::Invalid
            }
            ParseError::Engine(_) => ErrorKind::Other,
        };
        Self {
            reason: describe(err),
            stage: IngestStage::Extract,
            kind,
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

fn describe(err: impl Display) -> String {
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
    let stem = Path::new(name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    stem.replace(['_', '-'], " ").trim().to_string()
}

/// Where saving ends in a document's overall progress (embedding goes from `EMBED_START` to 1).
const SAVED: f32 = EMBED_START;

/// Records the status and reports the progress as each stage of the content pipeline starts.
struct Observer<'a> {
    ingestion: &'a DocumentIngestion,
    document: &'a DocumentRecord,
    stages: std::sync::Mutex<Vec<(PipelineStage, Instant)>>,
    started: Instant,
}

impl<'a> Observer<'a> {
    fn new(ingestion: &'a DocumentIngestion, document: &'a DocumentRecord) -> Self {
        Self {
            ingestion,
            document,
            stages: Default::default(),
            started: Instant::now(),
        }
    }

    /// Milliseconds spent parsing, normalizing and chunking.
    fn timings(&self) -> (u64, u64, u64) {
        let stages = self.stages.lock().unwrap();
        let end = Instant::now();
        let spent = |stage: PipelineStage| {
            let at = stages.iter().position(|(s, _)| *s == stage)?;
            let to = stages.get(at + 1).map_or(end, |(_, t)| *t);
            Some(ms_between(stages[at].1, to))
        };
        let _ = self.started;
        (
            spent(PipelineStage::Parsing).unwrap_or(0),
            spent(PipelineStage::Structuring).unwrap_or(0),
            spent(PipelineStage::Chunking).unwrap_or(0),
        )
    }
}

fn ms_between(from: Instant, to: Instant) -> u64 {
    to.duration_since(from).as_millis() as u64
}

impl StageObserver for Observer<'_> {
    fn started(&self, stage: PipelineStage) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.stages.lock().unwrap().push((stage, Instant::now()));
            let (status, phase, fraction) = match stage {
                PipelineStage::Parsing => (DocumentStatus::Extracting, IngestPhase::Parsing, 0.05),
                PipelineStage::Structuring => {
                    (DocumentStatus::Structuring, IngestPhase::Structuring, 0.3)
                }
                PipelineStage::Chunking => (DocumentStatus::Chunking, IngestPhase::Chunking, 0.45),
            };
            // Best effort: the status is for display; a failure here shows up at the save.
            if stage != PipelineStage::Parsing {
                let _ = self
                    .ingestion
                    .documents
                    .set_status(self.document.id, status, None)
                    .await;
            }
            self.ingestion
                .report(self.document, phase, fraction, None, status);
        })
    }
}
