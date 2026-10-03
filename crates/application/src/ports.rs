//! Port traits (`DocumentEngine`, `EmbeddingProvider`, `VectorStore`, `LlmProvider`, ...).
//!
//! Ports are object-safe so the composition root can inject `Arc<dyn Port>`; async operations
//! return [`BoxFuture`].

use std::{fmt, future::Future, path::Path, pin::Pin};

use nlmx_domain::{
    document::{
        DocumentError, DocumentHandle, DocumentMetadata, PageImage, PageInfo, RenderOptions,
        RenderedPage, TextSpan,
    },
    generation::{Generation, GenerationRequest, LanguageModelStatus, LlmCapabilities, LlmError},
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The answer-generating model (Apple Foundation Models in the app).
pub trait LlmProvider: Send + Sync {
    fn status(&self) -> BoxFuture<'_, LanguageModelStatus>;
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

// ── Ingestion ────────────────────────────────────────────────────────────────

use nlmx_domain::ingestion::{
    ChunkDraft, ChunkPolicy, DocumentId, DocumentStatus, DocumentSummary, PageLayout,
    StructuredDocument,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDigest {
    /// Lowercase hex SHA-256 of the file content.
    pub sha256: String,
    pub size: u64,
}

/// The on-disk document library.
pub trait FileStore: Send + Sync {
    fn digest<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<FileDigest, StorageError>>;
    /// Copies `path` into the library under its hash and returns the library path. Idempotent.
    fn store<'a>(
        &'a self,
        path: &'a Path,
        sha256: &'a str,
    ) -> BoxFuture<'a, Result<std::path::PathBuf, StorageError>>;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDocument {
    pub sha256: String,
    pub original_filename: String,
    pub original_path: String,
    pub library_path: String,
    pub file_size: u64,
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

pub trait DocumentRepository: Send + Sync {
    fn find_by_sha256<'a>(
        &'a self,
        sha256: &'a str,
    ) -> BoxFuture<'a, Result<Option<DocumentId>, StorageError>>;
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
    fn list(&self) -> BoxFuture<'_, Result<Vec<DocumentSummary>, StorageError>>;
    /// Documents whose ingestion was interrupted (see `DocumentStatus::is_unfinished`).
    fn unfinished(&self) -> BoxFuture<'_, Result<Vec<DocumentId>, StorageError>>;
    /// Page sizes saved by the last extraction, in page order (empty before extraction).
    fn pages(&self, id: DocumentId) -> BoxFuture<'_, Result<Vec<PageRecord>, StorageError>>;
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
    /// Position of the chunk in its document (consecutive ordinals are neighbours).
    pub ordinal: u32,
    pub content_hash: String,
    pub page_start: u32,
    pub page_end: u32,
    pub section: Option<String>,
    pub text: String,
    /// Where the chunk is on its pages (top-left origin, PDF points).
    pub bboxes: Vec<nlmx_domain::ingestion::PageBox>,
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
    Conversation, ConversationId, ConversationSummary, Message, MessageId, MessagePageRef,
    MessageSource, MessageStatus, Role,
};

/// Conversations, their messages and the sources of each answer.
pub trait ConversationRepository: Send + Sync {
    fn create(
        &self,
        document: Option<nlmx_domain::ingestion::DocumentId>,
    ) -> BoxFuture<'_, Result<Conversation, StorageError>>;
    fn get(&self, id: ConversationId) -> BoxFuture<'_, Result<Option<Conversation>, StorageError>>;
    /// Most recently updated first.
    fn recent(&self, limit: u32) -> BoxFuture<'_, Result<Vec<ConversationSummary>, StorageError>>;
    fn set_scope(
        &self,
        id: ConversationId,
        document: Option<nlmx_domain::ingestion::DocumentId>,
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
    /// Adds a message and touches the conversation.
    fn add_message<'a>(
        &'a self,
        conversation: ConversationId,
        role: Role,
        content: &'a str,
        status: MessageStatus,
    ) -> BoxFuture<'a, Result<MessageId, StorageError>>;
    /// Final content, status, error, sources and page references of an assistant message
    /// (previous ones replaced).
    fn finish_message<'a>(
        &'a self,
        id: MessageId,
        answer: FinishedAnswer<'a>,
    ) -> BoxFuture<'a, Result<(), StorageError>>;
    /// Back to `Streaming` with no content or sources (regeneration).
    fn reset_message(&self, id: MessageId) -> BoxFuture<'_, Result<(), StorageError>>;
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
