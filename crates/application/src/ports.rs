//! Port traits (`DocumentEngine`, `EmbeddingProvider`, `VectorStore`, `LlmProvider`, ...).
//!
//! Ports are object-safe so the composition root can inject `Arc<dyn Port>`; async operations
//! return [`BoxFuture`].

use std::{
    fmt,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use nlmx_domain::{
    document::{
        DocumentError, DocumentHandle, DocumentMetadata, PageImage, PageInfo, RenderOptions,
        RenderedPage, TextSpan,
    },
    document_type::DocumentType,
    generation::{Generation, GenerationRequest, LanguageModelStatus, LlmCapabilities, LlmError},
    parsed::{
        ChunkContext, ChunkMetadata as ParsedChunkMetadata, DocumentChunk,
        DocumentMetadata as ParsedMetadata, ParseError, ParsedDocument, SectionKind,
        SectionOutline,
    },
    source::SourceLocation,
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The answer-generating model (Apple Foundation Models in the app).
pub trait LlmProvider: Send + Sync {
    fn status(&self) -> BoxFuture<'_, LanguageModelStatus>;
    /// Asks the system again, ignoring any cached answer (e.g. after the user accepted the
    /// license). Providers without a cache just report their status.
    fn recheck(&self) -> BoxFuture<'_, LanguageModelStatus> {
        self.status()
    }
    fn capabilities(&self) -> LlmCapabilities;
    /// Exact token count of the request (instructions + user turn) as the model sees it.
    fn count_tokens<'a>(
        &'a self,
        request: &'a GenerationRequest,
    ) -> BoxFuture<'a, Result<u32, LlmError>>;
    /// Generates an answer, calling `on_token` with each piece of text as it arrives.
    /// Cancelling stops the generation and returns what was produced so far.
    fn generate<'a>(
        &'a self,
        request: &'a GenerationRequest,
        on_token: &'a (dyn Fn(&str) + Send + Sync),
        cancel: CancelFlag,
    ) -> BoxFuture<'a, Result<Generation, LlmError>>;
}

/// Failure of the local database. The message is suitable for logs and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageError {
    pub message: String,
}

impl StorageError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StorageError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageInfo {
    pub path: String,
    pub schema_version: u32,
    pub latest_schema_version: u32,
    pub sqlite_version: String,
    pub vector_extension_version: String,
}

/// Health and version information about the local database.
pub trait StorageDiagnostics: Send + Sync {
    fn info(&self) -> BoxFuture<'_, Result<StorageInfo, StorageError>>;
}

/// Key/value application settings; values are JSON documents.
pub trait SettingsRepository: Send + Sync {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Option<String>, StorageError>>;
    fn set<'a>(&'a self, key: &'a str, json: &'a str) -> BoxFuture<'a, Result<(), StorageError>>;
}

/// Reads and renders PDF documents. Pages are 1-based; coordinates use a top-left origin
/// (see `nlmx_domain::document`). Implementations must be safe to call from many tasks.
pub trait DocumentEngine: Send + Sync {
    fn open<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<DocumentHandle, DocumentError>>;
    fn close(&self, document: DocumentHandle) -> BoxFuture<'_, Result<(), DocumentError>>;
    fn page_count(&self, document: DocumentHandle) -> BoxFuture<'_, Result<u32, DocumentError>>;
    fn metadata(
        &self,
        document: DocumentHandle,
    ) -> BoxFuture<'_, Result<DocumentMetadata, DocumentError>>;
    fn page_info(
        &self,
        document: DocumentHandle,
        page: u32,
    ) -> BoxFuture<'_, Result<PageInfo, DocumentError>>;
    /// Pages whose text layer is empty — scanned, image-only or blank pages.
    fn pages_without_text(
        &self,
        document: DocumentHandle,
    ) -> BoxFuture<'_, Result<Vec<u32>, DocumentError>>;
    fn extract_text(
        &self,
        document: DocumentHandle,
        page: u32,
    ) -> BoxFuture<'_, Result<String, DocumentError>>;
    /// Text with positions and font information, for layout analysis and citation highlights.
    fn text_spans(
        &self,
        document: DocumentHandle,
        page: u32,
    ) -> BoxFuture<'_, Result<Vec<TextSpan>, DocumentError>>;
    fn page_images(
        &self,
        document: DocumentHandle,
        page: u32,
    ) -> BoxFuture<'_, Result<Vec<PageImage>, DocumentError>>;
    fn render_page(
        &self,
        document: DocumentHandle,
        page: u32,
        options: RenderOptions,
    ) -> BoxFuture<'_, Result<RenderedPage, DocumentError>>;
}

// ── Parsing ──────────────────────────────────────────────────────────────────

/// A file to parse and, when known, its format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSource {
    pub path: PathBuf,
    /// The declared format; when `None` it is read from the file extension.
    pub document_type: Option<DocumentType>,
    /// The text itself, for a source that has no file (a note): the path is then never read.
    pub text: Option<Arc<str>>,
}

impl DocumentSource {
    /// A file whose format is the one of its extension (`None` if it has no known one).
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let document_type = DocumentType::from_path(&path);
        Self {
            path,
            document_type,
            text: None,
        }
    }

    /// A file of a known format, whatever its extension.
    pub fn of_type(path: impl Into<PathBuf>, document_type: DocumentType) -> Self {
        Self {
            path: path.into(),
            document_type: Some(document_type),
            text: None,
        }
    }

    /// A note: the pasted text, with no file behind it.
    pub fn note(text: impl Into<Arc<str>>) -> Self {
        Self {
            path: PathBuf::new(),
            document_type: Some(DocumentType::Note),
            text: Some(text.into()),
        }
    }
}

/// Reads one format into the common [`ParsedDocument`]: metadata, structure and locations.
/// A parser knows nothing about chunking, embeddings or the RAG. Implementations must not
/// log the document's content, name or path.
pub trait DocumentParser: Send + Sync {
    fn document_type(&self) -> DocumentType;
    /// Version of the parser's output; bump it when the same file would parse differently.
    fn version(&self) -> u32;
    fn supports_type(&self, document_type: DocumentType) -> bool {
        document_type == self.document_type()
    }
    /// Whether the media type (e.g. `text/csv; charset=utf-8`) is one this parser reads.
    fn supports_mime(&self, mime: &str) -> bool {
        DocumentType::from_mime(mime).is_some_and(|kind| self.supports_type(kind))
    }
    fn parse<'a>(
        &'a self,
        source: &'a DocumentSource,
    ) -> BoxFuture<'a, Result<ParsedDocument, ParseError>>;
}

// ── Notes ────────────────────────────────────────────────────────────────────

/// Why a note was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteSubmitError {
    /// Empty, only whitespace, or too long.
    Invalid(nlmx_domain::note::NoteError),
    /// Importing is not available (no worker or no library).
    Unavailable,
}

impl fmt::Display for NoteSubmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => error.fmt(f),
            Self::Unavailable => f.write_str("Não foi possível adicionar a nota agora."),
        }
    }
}

/// Hands a pasted note to the import queue: it is validated at once and registered, indexed
/// and reported like any other source. The interface never touches the pipeline itself.
pub trait NoteSubmitter: Send + Sync {
    fn submit(&self, text: &str) -> Result<(), NoteSubmitError>;
}

// ── Ingestion ────────────────────────────────────────────────────────────────

use nlmx_domain::ingestion::{
    ChunkDraft, ChunkPolicy, DocumentId, DocumentStatus, DocumentSummary, IngestProgress,
    PageLayout, RemovalImpact, SourceDetails, StructuredDocument,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDigest {
    /// Lowercase hex SHA-256 of the file content.
    pub sha256: String,
    pub size: u64,
}

/// The on-disk document library.
pub trait FileStore: Send + Sync {
    /// The digest of a note's text. It never equals the digest of a file with the same bytes,
    /// because a note and a file are different documents.
    fn digest_text(&self, text: &str) -> FileDigest;
    fn digest<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<FileDigest, StorageError>>;
    /// Copies `path` into the library under its hash and returns the library path. Idempotent.
    fn store<'a>(
        &'a self,
        path: &'a Path,
        sha256: &'a str,
    ) -> BoxFuture<'a, Result<std::path::PathBuf, StorageError>>;
    /// Deletes the library copy of a document. A missing file is not an error.
    fn remove<'a>(&'a self, sha256: &'a str) -> BoxFuture<'a, Result<(), StorageError>>;
    /// Deletes library files (and leftover temporary copies) whose hash is not in `keep`;
    /// returns how many were deleted.
    fn prune(&self, keep: Vec<String>) -> BoxFuture<'_, Result<u32, StorageError>>;
}

/// Receives the progress of the indexing pipeline (the Tauri adapter forwards it to the
/// interface). Reports are best effort and must never block or fail the pipeline.
pub trait ProgressSink: Send + Sync {
    fn report(&self, progress: IngestProgress);
}

/// A sink that drops every report.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn report(&self, _progress: IngestProgress) {}
}

/// Turns page layouts into normalized blocks with headings and sections. Pure and deterministic.
pub trait StructureAnalyzer: Send + Sync {
    /// Bumped whenever the output for the same input changes (stored as `extractor_version`).
    fn version(&self) -> u32;
    fn analyze(&self, pages: &[PageLayout]) -> StructuredDocument;
}

/// Approximate token count of a text.
pub trait TokenCounter: Send + Sync {
    fn count(&self, text: &str) -> u32;
}

/// Splits a structured document into chunks. Pure and deterministic.
pub trait Chunker: Send + Sync {
    /// Bumped whenever the output for the same input changes (stored as `chunker_version`).
    fn version(&self) -> u32;
    fn chunk(
        &self,
        document: &StructuredDocument,
        policy: &ChunkPolicy,
        tokens: &dyn TokenCounter,
    ) -> Vec<ChunkDraft>;
}

/// Cleans the text of a parsed document, whatever its format: Unicode form, stray control and
/// zero-width characters, spacing. Pure and idempotent. It never changes the structure (blocks
/// keep their kind) or any location, only the text of blocks and titles; a block left empty
/// is dropped.
pub trait DocumentNormalizer: Send + Sync {
    /// Bumped whenever the output for the same input changes.
    fn version(&self) -> u32;
    fn normalize(&self, document: ParsedDocument) -> ParsedDocument;
}

/// Splits a (normalized) parsed document into chunks that keep the section path and the
/// source location of what they hold. Pure and deterministic; no parser details and no
/// embeddings here.
pub trait DocumentChunker: Send + Sync {
    /// Bumped whenever the output for the same input changes (stored as `chunker_version`).
    fn version(&self) -> u32;
    fn chunk(
        &self,
        document: &ParsedDocument,
        context: &ChunkContext,
        policy: &ChunkPolicy,
        tokens: &dyn TokenCounter,
    ) -> Vec<DocumentChunk>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDocument {
    pub sha256: String,
    pub original_filename: String,
    pub original_path: String,
    pub library_path: String,
    pub file_size: u64,
    /// The format of the file; its media type is derived from it.
    pub document_type: DocumentType,
    /// The text of a note (`DocumentType::Note`); `None` for every document that has a file.
    pub note_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    Inserted(DocumentId),
    /// A document with the same content hash already exists.
    AlreadyExists(DocumentId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentRecord {
    pub id: DocumentId,
    pub sha256: String,
    pub original_filename: String,
    pub library_path: String,
    pub status: DocumentStatus,
    pub file_size: u64,
    pub document_type: DocumentType,
    pub mime_type: String,
    /// The text of a note; `None` for a document that has a file.
    pub note_text: Option<String>,
}

impl DocumentRecord {
    /// Whether the interface can open the document (only a PDF can).
    pub fn previewable(&self) -> bool {
        self.document_type.previewable()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PageRecord {
    pub number: u32,
    pub width: f32,
    pub height: f32,
    pub char_count: u32,
    pub has_text: bool,
}

/// Everything the pipeline produced for one document, saved atomically.
#[derive(Debug, Clone, PartialEq)]
pub struct Extraction {
    pub title: String,
    pub author: Option<String>,
    pub pdf_created_at: Option<String>,
    pub page_count: u32,
    pub has_text_layer: bool,
    pub pages: Vec<PageRecord>,
    pub chunks: Vec<ChunkDraft>,
    pub extractor_version: u32,
    pub chunker_version: u32,
    /// Status after saving: `Embedding` (waiting for embeddings) or `NeedsOcr`.
    pub status: DocumentStatus,
}

/// Everything the content pipeline produced for one document of any format, saved atomically
/// (see `DocumentRepository::save_processed`). Chunks must be of the document's format.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredExtraction {
    pub title: String,
    pub metadata: ParsedMetadata,
    /// Pages of a paged format (empty otherwise).
    pub pages: Vec<PageRecord>,
    pub outline: Vec<SectionOutline>,
    pub chunks: Vec<DocumentChunk>,
    pub extractor_version: u32,
    pub normalizer_version: u32,
    pub chunker_version: u32,
    /// Status after saving: `Embedding` (waiting for embeddings) or `NeedsOcr`.
    pub status: DocumentStatus,
}

/// A stored section of a document's structure.
#[derive(Debug, Clone, PartialEq)]
pub struct SectionRecord {
    pub id: i64,
    /// The enclosing section, if any.
    pub parent_id: Option<i64>,
    pub kind: SectionKind,
    pub title: Option<String>,
    pub level: u8,
    pub ordinal: u32,
    /// Enclosing headings including the section's own, outermost first.
    pub path: Vec<String>,
    pub location: SourceLocation,
}

pub trait DocumentRepository: Send + Sync {
    fn find_by_sha256<'a>(
        &'a self,
        sha256: &'a str,
    ) -> BoxFuture<'a, Result<Option<DocumentId>, StorageError>>;
    /// The most recently imported document whose original file was at `path`.
    fn find_by_original_path<'a>(
        &'a self,
        path: &'a str,
    ) -> BoxFuture<'a, Result<Option<DocumentId>, StorageError>>;
    /// Points an existing document at a new version of its file (new hash, size, library copy,
    /// format) and queues it again. Its chunks stay until the next `save_processed` replaces them.
    fn replace_source(
        &self,
        id: DocumentId,
        source: NewDocument,
    ) -> BoxFuture<'_, Result<(), StorageError>>;
    /// Inserts a queued document and its ingest job; reports an existing document with the same hash.
    fn insert(&self, document: NewDocument) -> BoxFuture<'_, Result<InsertOutcome, StorageError>>;
    fn get(&self, id: DocumentId) -> BoxFuture<'_, Result<Option<DocumentRecord>, StorageError>>;
    fn set_status(
        &self,
        id: DocumentId,
        status: DocumentStatus,
        error: Option<String>,
    ) -> BoxFuture<'_, Result<(), StorageError>>;
    /// Replaces metadata, pages and chunks in one transaction (idempotent re-ingestion).
    fn save_extraction(
        &self,
        id: DocumentId,
        extraction: Extraction,
    ) -> BoxFuture<'_, Result<(), StorageError>>;
    /// Like `save_extraction`, for a document of any format: also the metadata, the structure
    /// and each chunk's location (a PDF keeps its page and boxes where the viewer reads them).
    /// Fails, saving nothing, if a chunk is of another format than the document.
    fn save_processed(
        &self,
        id: DocumentId,
        extraction: StoredExtraction,
    ) -> BoxFuture<'_, Result<(), StorageError>>;
    /// The stored chunks in order, with their location whatever the format. `ChunkDraft`-era
    /// PDF chunks read as `SourceLocation::Pdf`.
    fn chunks_of(&self, id: DocumentId) -> BoxFuture<'_, Result<Vec<DocumentChunk>, StorageError>>;
    /// The stored structure in reading order (empty for documents saved before sections existed).
    fn sections_of(
        &self,
        id: DocumentId,
    ) -> BoxFuture<'_, Result<Vec<SectionRecord>, StorageError>>;
    fn list(&self) -> BoxFuture<'_, Result<Vec<DocumentSummary>, StorageError>>;
    /// What the interface shows about one source (`None` if it does not exist).
    fn source_details(
        &self,
        id: DocumentId,
    ) -> BoxFuture<'_, Result<Option<SourceDetails>, StorageError>>;
    /// Documents whose ingestion was interrupted (see `DocumentStatus::is_unfinished`).
    fn unfinished(&self) -> BoxFuture<'_, Result<Vec<DocumentId>, StorageError>>;
    /// Page sizes saved by the last extraction, in page order (empty before extraction).
    fn pages(&self, id: DocumentId) -> BoxFuture<'_, Result<Vec<PageRecord>, StorageError>>;
    /// What `remove` would delete (all zero for an unknown document).
    fn removal_impact(&self, id: DocumentId) -> BoxFuture<'_, Result<RemovalImpact, StorageError>>;
    /// Deletes the document, everything derived from it and the chat history that used it, in
    /// one transaction (see `RemovalImpact`). `None` if there is no such document.
    fn remove(
        &self,
        id: DocumentId,
    ) -> BoxFuture<'_, Result<Option<RemovedDocument>, StorageError>>;
    /// Content hashes of every document, to find library files nobody references.
    fn hashes(&self) -> BoxFuture<'_, Result<Vec<String>, StorageError>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedDocument {
    pub sha256: String,
    pub impact: RemovalImpact,
}

/// Read-only view of the ingestion jobs and the vector index, for the Indexação section.
pub trait IndexingReader: Send + Sync {
    /// Every document with its latest ingest job, the total chunk count and, for `model_id`
    /// (the active embedding model's id), how many chunks have a vector.
    fn snapshot(
        &self,
        model_id: Option<String>,
    ) -> BoxFuture<'_, Result<nlmx_domain::indexing::IndexSnapshot, StorageError>>;
}

// ── Embeddings ───────────────────────────────────────────────────────────────

use nlmx_domain::embedding::{EmbeddingError, EmbeddingPurpose, ModelIdentity};

/// The embedding model currently in use, which can change while the app runs (a model is
/// downloaded, activated or removed). `None` when no model is configured.
pub trait EmbeddingSource: Send + Sync {
    fn current(&self) -> Option<std::sync::Arc<dyn EmbeddingProvider>>;
}

/// Turns text into L2-normalized vectors with a fixed dimension.
pub trait EmbeddingProvider: Send + Sync {
    /// Stable model id (from configuration); available without contacting the model.
    fn model_id(&self) -> &str;
    /// Full identity, including the dimension reported by the loaded model.
    fn identity(&self) -> BoxFuture<'_, Result<ModelIdentity, EmbeddingError>>;
    fn dimensions(&self) -> BoxFuture<'_, Result<u32, EmbeddingError>>;
    fn embed<'a>(
        &'a self,
        text: &'a str,
        purpose: EmbeddingPurpose,
    ) -> BoxFuture<'a, Result<Vec<f32>, EmbeddingError>>;
    /// One vector per input, in the same order.
    fn embed_batch<'a>(
        &'a self,
        texts: &'a [String],
        purpose: EmbeddingPurpose,
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, EmbeddingError>>;
}

// ── Runtime and models ───────────────────────────────────────────────────────

use nlmx_domain::models::{
    ConfirmedDownload, DiskSpace, DownloadPlan, DownloadProgress, InstalledModel, ModelDescriptor,
    ModelError, ModelState, RuntimeInfo, VerifyReport,
};

/// The inference runtime bundled with the app (detected, never downloaded).
pub trait InferenceRuntime: Send + Sync {
    fn info(&self) -> BoxFuture<'_, Result<RuntimeInfo, String>>;
}

/// Cooperative cancellation for long operations such as downloads.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl CancelFlag {
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

pub type ProgressCallback = std::sync::Arc<dyn Fn(DownloadProgress) + Send + Sync>;

/// Model files: catalog, installed models, downloads, verification and removal.
pub trait ModelProvider: Send + Sync {
    fn catalog(&self) -> Vec<ModelDescriptor>;
    fn installed(&self) -> BoxFuture<'_, Result<Vec<InstalledModel>, ModelError>>;
    fn status<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ModelState, ModelError>>;
    /// Describes a download (size, license, disk) for the user to confirm. Downloads nothing.
    fn plan_download<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<DownloadPlan, ModelError>>;
    /// Downloads only what the user confirmed (see `DownloadPlan::confirm`).
    fn download(
        &self,
        confirmed: ConfirmedDownload,
        progress: ProgressCallback,
        cancel: CancelFlag,
    ) -> BoxFuture<'_, Result<InstalledModel, ModelError>>;
    /// Full SHA-256 check of an installed model.
    fn verify<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<VerifyReport, ModelError>>;
    fn remove<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), ModelError>>;
    /// Plan for installing the catalog's newer version of an installed model.
    fn plan_update<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<DownloadPlan, ModelError>>;
    /// Makes an installed model the one used for embeddings.
    fn activate<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<InstalledModel, ModelError>>;
    fn active(&self) -> BoxFuture<'_, Result<Option<InstalledModel>, ModelError>>;
    fn disk(&self) -> BoxFuture<'_, Result<DiskSpace, ModelError>>;
}

// ── Vector store ─────────────────────────────────────────────────────────────

use nlmx_domain::vectors::{
    ChunkId, DeleteScope, EmbeddingSpace, VectorError, VectorFilter, VectorHit, VectorIndex,
    VectorIndexId,
};

/// Vectors of chunks, one index per embedding space (model + revision + dimension).
pub trait VectorStore: Send + Sync {
    /// Creates (or returns the existing) index for a space. The same model/revision with a
    /// different dimension is an error.
    fn create_index<'a>(
        &'a self,
        space: &'a EmbeddingSpace,
    ) -> BoxFuture<'a, Result<VectorIndex, VectorError>>;
    /// Inserts or replaces the vector of a chunk.
    fn insert_embedding<'a>(
        &'a self,
        index: VectorIndexId,
        chunk: ChunkId,
        vector: &'a [f32],
    ) -> BoxFuture<'a, Result<(), VectorError>>;
    /// All or nothing.
    fn insert_batch<'a>(
        &'a self,
        index: VectorIndexId,
        items: &'a [(ChunkId, Vec<f32>)],
    ) -> BoxFuture<'a, Result<(), VectorError>>;
    /// The `k` nearest chunks by cosine distance, closest first.
    fn search<'a>(
        &'a self,
        index: VectorIndexId,
        query: &'a [f32],
        k: usize,
        filter: &'a VectorFilter,
    ) -> BoxFuture<'a, Result<Vec<VectorHit>, VectorError>>;
    /// Returns how many vectors were removed.
    fn delete<'a>(
        &'a self,
        index: VectorIndexId,
        scope: &'a DeleteScope,
    ) -> BoxFuture<'a, Result<u64, VectorError>>;
    /// Recreates the index storage from its current vectors (compaction/repair). Returns the count.
    fn rebuild(&self, index: VectorIndexId) -> BoxFuture<'_, Result<u64, VectorError>>;
    fn count(&self, index: VectorIndexId) -> BoxFuture<'_, Result<u64, VectorError>>;
}

// ── Lexical search and chunk reading ─────────────────────────────────────────

use nlmx_domain::retrieval::{LexicalCandidate, LexicalQuery, RetrievalFilter};

/// Full-text (BM25) search over chunk text.
pub trait LexicalIndex: Send + Sync {
    /// Best `k` matches, best first. An empty query returns nothing.
    fn search<'a>(
        &'a self,
        query: &'a LexicalQuery,
        k: usize,
        filter: &'a RetrievalFilter,
    ) -> BoxFuture<'a, Result<Vec<LexicalCandidate>, StorageError>>;
}

/// A filter resolved to what each mechanism can apply directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Candidates {
    /// Documents allowed by the document and collection filters (`None` = all).
    pub documents: Option<Vec<nlmx_domain::ingestion::DocumentId>>,
    /// Chunks allowed by the page filter (`None` = all).
    pub chunks: Option<Vec<ChunkId>>,
}

/// What retrieval returns to the caller about a chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkView {
    pub chunk_id: ChunkId,
    pub document_id: nlmx_domain::ingestion::DocumentId,
    pub document_title: String,
    /// The name of the file as it was imported.
    pub document_name: String,
    pub document_type: DocumentType,
    /// Position of the chunk in its document (consecutive ordinals are neighbours).
    pub ordinal: u32,
    pub content_hash: String,
    /// Pages of a PDF chunk. Meaningless for any other format: read `location`.
    pub page_start: u32,
    pub page_end: u32,
    pub section: Option<String>,
    pub text: String,
    /// Where the chunk is on its pages (top-left origin, PDF points); PDF only.
    pub bboxes: Vec<nlmx_domain::ingestion::PageBox>,
    /// Where the chunk comes from, whatever the format. The source of truth for provenance.
    pub location: SourceLocation,
    pub metadata: ParsedChunkMetadata,
}

pub trait ChunkReader: Send + Sync {
    fn resolve<'a>(
        &'a self,
        filter: &'a RetrievalFilter,
    ) -> BoxFuture<'a, Result<Candidates, StorageError>>;
    /// All chunks of a document, in reading order.
    fn document_chunks(
        &self,
        document: nlmx_domain::ingestion::DocumentId,
    ) -> BoxFuture<'_, Result<Vec<ChunkView>, StorageError>>;
    /// Chunks by id, in the order requested (missing ids are skipped).
    fn get_many<'a>(
        &'a self,
        ids: &'a [ChunkId],
    ) -> BoxFuture<'a, Result<Vec<ChunkView>, StorageError>>;
    /// Documents that have chunks (searchable), ascending.
    fn chunked_documents(
        &self,
    ) -> BoxFuture<'_, Result<Vec<nlmx_domain::ingestion::DocumentId>, StorageError>>;
}

// ── Conversations ────────────────────────────────────────────────────────────

use nlmx_domain::chat::{
    AnswerGrounding, Conversation, ConversationId, ConversationScope, ConversationSummary, Message,
    MessageId, MessagePageRef, MessageSource, MessageStatus, Role,
};

/// Conversations, their messages and the sources of each answer.
pub trait ConversationRepository: Send + Sync {
    fn create(&self, scope: ConversationScope)
    -> BoxFuture<'_, Result<Conversation, StorageError>>;
    fn get(&self, id: ConversationId) -> BoxFuture<'_, Result<Option<Conversation>, StorageError>>;
    /// Most recently updated first.
    fn recent(&self, limit: u32) -> BoxFuture<'_, Result<Vec<ConversationSummary>, StorageError>>;
    fn set_scope(
        &self,
        id: ConversationId,
        scope: ConversationScope,
    ) -> BoxFuture<'_, Result<(), StorageError>>;
    fn set_title<'a>(
        &'a self,
        id: ConversationId,
        title: &'a str,
    ) -> BoxFuture<'a, Result<(), StorageError>>;
    fn delete(&self, id: ConversationId) -> BoxFuture<'_, Result<(), StorageError>>;
    /// Oldest first, with their sources.
    fn messages(&self, id: ConversationId) -> BoxFuture<'_, Result<Vec<Message>, StorageError>>;
    fn message(&self, id: MessageId) -> BoxFuture<'_, Result<Option<Message>, StorageError>>;
    /// Adds a message and touches the conversation. `grounding` is for assistant messages.
    fn add_message<'a>(
        &'a self,
        conversation: ConversationId,
        role: Role,
        content: &'a str,
        status: MessageStatus,
        grounding: Option<AnswerGrounding>,
    ) -> BoxFuture<'a, Result<MessageId, StorageError>>;
    /// Final content, status, error, sources and page references of an assistant message
    /// (previous ones replaced).
    fn finish_message<'a>(
        &'a self,
        id: MessageId,
        answer: FinishedAnswer<'a>,
    ) -> BoxFuture<'a, Result<(), StorageError>>;
    /// Back to `Streaming` with no content or sources, to be generated with `grounding`
    /// (regeneration).
    fn reset_message(
        &self,
        id: MessageId,
        grounding: AnswerGrounding,
    ) -> BoxFuture<'_, Result<(), StorageError>>;
}

/// What `ConversationRepository::finish_message` saves.
#[derive(Debug, Clone, Copy)]
pub struct FinishedAnswer<'a> {
    pub content: &'a str,
    pub status: MessageStatus,
    pub error: Option<&'a str>,
    pub sources: &'a [MessageSource],
    pub page_refs: &'a [MessagePageRef],
}

// ── Diagnostics ──────────────────────────────────────────────────────────────

use nlmx_domain::telemetry::Operation;

/// Duration and outcome statistics of one operation in this session.
#[derive(Debug, Clone, PartialEq)]
pub struct OperationStats {
    pub operation: Operation,
    pub total: u64,
    pub failed: u64,
    pub p50_ms: Option<u64>,
    pub p95_ms: Option<u64>,
    pub max_ms: Option<u64>,
}

impl OperationStats {
    /// Failed ÷ total (`None` before the first operation).
    pub fn error_rate(&self) -> Option<f64> {
        (self.total > 0).then(|| self.failed as f64 / self.total as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResourceUsage {
    pub rss_bytes: u64,
    pub llama_rss_bytes: Option<u64>,
    pub fm_rss_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StorageUsage {
    pub database_bytes: u64,
    pub library_bytes: u64,
    pub models_bytes: u64,
    pub logs_bytes: u64,
}

impl StorageUsage {
    pub fn total(&self) -> u64 {
        self.database_bytes + self.library_bytes + self.models_bytes + self.logs_bytes
    }
}

/// Aggregated measurements of the session. Contains no document content by construction.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DiagnosticsSnapshot {
    pub uptime_secs: u64,
    pub operations: Vec<OperationStats>,
    pub documents_imported: u64,
    pub pages: u64,
    pub chunks: u64,
    pub pages_per_second: Option<f64>,
    pub embeddings: u64,
    pub embeddings_per_second: Option<f64>,
    pub first_token_p50_ms: Option<u64>,
    pub resources: Option<ResourceUsage>,
    pub storage: Option<StorageUsage>,
}

impl DiagnosticsSnapshot {
    pub fn operation(&self, operation: Operation) -> Option<&OperationStats> {
        self.operations.iter().find(|o| o.operation == operation)
    }
}

/// Session metrics, memory and storage, for the diagnostics screen.
pub trait Diagnostics: Send + Sync {
    fn snapshot(&self) -> DiagnosticsSnapshot;
    /// Samples memory and storage now (they are otherwise sampled periodically).
    fn sample(&self) -> BoxFuture<'_, ()>;
}
