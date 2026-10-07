//! Test support: in-memory fakes of every port and reusable contract suites
//! (e.g. `vector_index_contract(impl)`) that each adapter must pass.

use std::{collections::HashMap, sync::Mutex};

mod parsing;
mod pipeline;
mod store;

pub use parsing::*;
pub use pipeline::*;
pub use store::*;

use nlmx_application::ports::{
    BoxFuture, CancelFlag, DocumentEngine, LlmProvider, SettingsRepository, StorageDiagnostics,
    StorageError, StorageInfo,
};
use nlmx_domain::{
    document::{
        DocumentError, DocumentHandle, DocumentMetadata, PageImage, PageInfo, RenderOptions,
        RenderedPage, TextSpan,
    },
    generation::{
        FinishReason, Generation, GenerationRequest, LanguageModelStatus, LlmCapabilities, LlmError,
    },
};

/// A scripted language model: fixed status, a canned answer (streamed word by word) or error,
/// token counts of ≈ 4 characters per token unless scripted, and a record of every request.
pub struct FakeLlmProvider {
    status: Mutex<LanguageModelStatus>,
    rechecks: Mutex<usize>,
    context_tokens: u32,
    answer: Result<String, LlmError>,
    token_counts: Mutex<Vec<u32>>,
    /// Cancel after this many streamed pieces.
    cancel_after: Option<usize>,
    requests: Mutex<Vec<GenerationRequest>>,
    counted: Mutex<usize>,
}

impl FakeLlmProvider {
    pub fn new(status: LanguageModelStatus) -> Self {
        Self {
            status: Mutex::new(status),
            rechecks: Mutex::new(0),
            context_tokens: 4096,
            answer: Ok(String::new()),
            token_counts: Mutex::new(Vec::new()),
            cancel_after: None,
            requests: Mutex::new(Vec::new()),
            counted: Mutex::new(0),
        }
    }

    pub fn available() -> Self {
        Self::new(LanguageModelStatus::Available)
    }

    pub fn answering(mut self, answer: &str) -> Self {
        self.answer = Ok(answer.to_string());
        self
    }

    pub fn failing(mut self, error: LlmError) -> Self {
        self.answer = Err(error);
        self
    }

    pub fn context_tokens(mut self, tokens: u32) -> Self {
        self.context_tokens = tokens;
        self
    }

    /// Successive `count_tokens` results (then back to the estimate).
    pub fn token_counts(self, counts: Vec<u32>) -> Self {
        *self.token_counts.lock().unwrap() = counts;
        self
    }

    pub fn cancel_after(mut self, pieces: usize) -> Self {
        self.cancel_after = Some(pieces);
        self
    }

    /// Requests passed to `generate`.
    pub fn requests(&self) -> Vec<GenerationRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn count_calls(&self) -> usize {
        *self.counted.lock().unwrap()
    }

    /// Changes what the system reports from now on (e.g. the user accepted the license).
    pub fn set_status(&self, status: LanguageModelStatus) {
        *self.status.lock().unwrap() = status;
    }

    /// How many times `recheck` was called.
    pub fn recheck_calls(&self) -> usize {
        *self.rechecks.lock().unwrap()
    }
}

impl LlmProvider for FakeLlmProvider {
    fn status(&self) -> BoxFuture<'_, LanguageModelStatus> {
        let status = self.status.lock().unwrap().clone();
        Box::pin(async move { status })
    }

    fn recheck(&self) -> BoxFuture<'_, LanguageModelStatus> {
        *self.rechecks.lock().unwrap() += 1;
        self.status()
    }

    fn capabilities(&self) -> LlmCapabilities {
        LlmCapabilities {
            context_tokens: self.context_tokens,
        }
    }

    fn count_tokens<'a>(
        &'a self,
        request: &'a GenerationRequest,
    ) -> BoxFuture<'a, Result<u32, LlmError>> {
        Box::pin(async move {
            *self.counted.lock().unwrap() += 1;
            let mut scripted = self.token_counts.lock().unwrap();
            if !scripted.is_empty() {
                return Ok(scripted.remove(0));
            }
            let chars = request.system.chars().count() + request.flat_user().chars().count();
            Ok((chars as u32).div_ceil(4))
        })
    }

    fn generate<'a>(
        &'a self,
        request: &'a GenerationRequest,
        on_token: &'a (dyn Fn(&str) + Send + Sync),
        cancel: CancelFlag,
    ) -> BoxFuture<'a, Result<Generation, LlmError>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request.clone());
            let answer = self.answer.clone()?;
            let mut text = String::new();
            for (i, piece) in answer.split_inclusive(' ').enumerate() {
                if self.cancel_after == Some(i) {
                    cancel.cancel();
                }
                if cancel.is_cancelled() {
                    return Ok(Generation {
                        text,
                        finish: FinishReason::Cancelled,
                    });
                }
                on_token(piece);
                text.push_str(piece);
            }
            Ok(Generation {
                text,
                finish: FinishReason::Completed,
            })
        })
    }
}

/// In-memory storage: settings in a map, diagnostics fixed (healthy or failing).
pub struct FakeStorage {
    info: Result<StorageInfo, StorageError>,
    settings: Mutex<HashMap<String, String>>,
}

impl FakeStorage {
    pub fn healthy() -> Self {
        Self::with_info(Ok(StorageInfo {
            path: "/tmp/nlmx-test.sqlite3".into(),
            schema_version: 5,
            latest_schema_version: 5,
            sqlite_version: "3.0.0".into(),
            vector_extension_version: "v0.0.0".into(),
        }))
    }

    pub fn failing(message: &str) -> Self {
        Self::with_info(Err(StorageError::new(message)))
    }

    fn with_info(info: Result<StorageInfo, StorageError>) -> Self {
        Self {
            info,
            settings: Mutex::default(),
        }
    }
}

impl StorageDiagnostics for FakeStorage {
    fn info(&self) -> BoxFuture<'_, Result<StorageInfo, StorageError>> {
        let info = self.info.clone();
        Box::pin(async move { info })
    }
}

impl SettingsRepository for FakeStorage {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Option<String>, StorageError>> {
        Box::pin(async move { Ok(self.settings.lock().unwrap().get(key).cloned()) })
    }

    fn set<'a>(&'a self, key: &'a str, json: &'a str) -> BoxFuture<'a, Result<(), StorageError>> {
        Box::pin(async move {
            self.settings
                .lock()
                .unwrap()
                .insert(key.to_string(), json.to_string());
            Ok(())
        })
    }
}

/// An in-memory page for [`FakeDocumentEngine`].
#[derive(Debug, Clone, Default)]
pub struct FakePage {
    pub spans: Vec<TextSpan>,
    pub images: Vec<PageImage>,
}

/// Document engine over in-memory documents registered by path.
#[derive(Default)]
pub struct FakeDocumentEngine {
    documents: Mutex<HashMap<std::path::PathBuf, (DocumentMetadata, Vec<FakePage>)>>,
    open: Mutex<HashMap<u64, std::path::PathBuf>>,
    open_errors: Mutex<HashMap<std::path::PathBuf, DocumentError>>,
    next: std::sync::atomic::AtomicU64,
}

impl FakeDocumentEngine {
    pub fn with_document(
        self,
        path: impl Into<std::path::PathBuf>,
        metadata: DocumentMetadata,
        pages: Vec<FakePage>,
    ) -> Self {
        self.documents
            .lock()
            .unwrap()
            .insert(path.into(), (metadata, pages));
        self
    }

    /// Documents opened and not yet closed.
    pub fn open_documents(&self) -> usize {
        self.open.lock().unwrap().len()
    }

    /// `open` of this path fails with `error` (e.g. a corrupt or password-protected file).
    pub fn with_open_error(
        self,
        path: impl Into<std::path::PathBuf>,
        error: DocumentError,
    ) -> Self {
        self.open_errors.lock().unwrap().insert(path.into(), error);
        self
    }

    fn with_doc<T>(
        &self,
        handle: DocumentHandle,
        f: impl FnOnce(&DocumentMetadata, &[FakePage]) -> Result<T, DocumentError>,
    ) -> Result<T, DocumentError> {
        let path = self
            .open
            .lock()
            .unwrap()
            .get(&handle.0)
            .cloned()
            .ok_or(DocumentError::UnknownHandle)?;
        let documents = self.documents.lock().unwrap();
        let (metadata, pages) = documents.get(&path).ok_or(DocumentError::UnknownHandle)?;
        f(metadata, pages)
    }

    fn with_page<T>(
        &self,
        handle: DocumentHandle,
        page: u32,
        f: impl FnOnce(&FakePage) -> T,
    ) -> Result<T, DocumentError> {
        self.with_doc(handle, |_, pages| {
            let page_count = pages.len() as u32;
            pages
                .get((page as usize).wrapping_sub(1))
                .map(f)
                .ok_or(DocumentError::PageOutOfRange { page, page_count })
        })
    }
}

fn fake_text(page: &FakePage) -> String {
    page.spans
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

impl DocumentEngine for FakeDocumentEngine {
    fn open<'a>(
        &'a self,
        path: &'a std::path::Path,
    ) -> BoxFuture<'a, Result<DocumentHandle, DocumentError>> {
        Box::pin(async move {
            if let Some(error) = self.open_errors.lock().unwrap().get(path) {
                return Err(error.clone());
            }
            if !self.documents.lock().unwrap().contains_key(path) {
                return Err(DocumentError::NotFound);
            }
            let id = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            self.open.lock().unwrap().insert(id, path.to_path_buf());
            Ok(DocumentHandle(id))
        })
    }

    fn close(&self, document: DocumentHandle) -> BoxFuture<'_, Result<(), DocumentError>> {
        Box::pin(async move {
            self.open
                .lock()
                .unwrap()
                .remove(&document.0)
                .map(drop)
                .ok_or(DocumentError::UnknownHandle)
        })
    }

    fn page_count(&self, document: DocumentHandle) -> BoxFuture<'_, Result<u32, DocumentError>> {
        Box::pin(async move { self.with_doc(document, |_, pages| Ok(pages.len() as u32)) })
    }

    fn metadata(
        &self,
        document: DocumentHandle,
    ) -> BoxFuture<'_, Result<DocumentMetadata, DocumentError>> {
        Box::pin(async move {
            self.with_doc(document, |meta, pages| {
                Ok(DocumentMetadata {
                    page_count: pages.len() as u32,
                    ..meta.clone()
                })
            })
        })
    }

    fn page_info(
        &self,
        document: DocumentHandle,
        page: u32,
    ) -> BoxFuture<'_, Result<PageInfo, DocumentError>> {
        Box::pin(async move {
            self.with_page(document, page, |p| {
                let char_count = fake_text(p).chars().filter(|c| !c.is_whitespace()).count() as u32;
                PageInfo {
                    number: page,
                    width: 612.0,
                    height: 792.0,
                    char_count,
                    image_count: p.images.len() as u32,
                    has_text: char_count > 0,
                }
            })
        })
    }

    fn pages_without_text(
        &self,
        document: DocumentHandle,
    ) -> BoxFuture<'_, Result<Vec<u32>, DocumentError>> {
        Box::pin(async move {
            self.with_doc(document, |_, pages| {
                Ok((1..=pages.len() as u32)
                    .filter(|&n| fake_text(&pages[n as usize - 1]).trim().is_empty())
                    .collect())
            })
        })
    }

    fn extract_text(
        &self,
        document: DocumentHandle,
        page: u32,
    ) -> BoxFuture<'_, Result<String, DocumentError>> {
        Box::pin(async move { self.with_page(document, page, fake_text) })
    }

    fn text_spans(
        &self,
        document: DocumentHandle,
        page: u32,
    ) -> BoxFuture<'_, Result<Vec<TextSpan>, DocumentError>> {
        Box::pin(async move { self.with_page(document, page, |p| p.spans.clone()) })
    }

    fn page_images(
        &self,
        document: DocumentHandle,
        page: u32,
    ) -> BoxFuture<'_, Result<Vec<PageImage>, DocumentError>> {
        Box::pin(async move { self.with_page(document, page, |p| p.images.clone()) })
    }

    fn render_page(
        &self,
        document: DocumentHandle,
        page: u32,
        options: RenderOptions,
    ) -> BoxFuture<'_, Result<RenderedPage, DocumentError>> {
        Box::pin(async move {
            self.with_page(document, page, |_| RenderedPage {
                png: Vec::new(),
                width_px: (612.0 * options.scale) as u32,
                height_px: (792.0 * options.scale) as u32,
            })
        })
    }
}

// ── Ingestion fakes ──────────────────────────────────────────────────────────

use nlmx_application::ports::{
    Chunker, DocumentRecord, DocumentRepository, Extraction, FileDigest, FileStore, InsertOutcome,
    NewDocument, RemovedDocument, SectionRecord, StoredExtraction, StructureAnalyzer, TokenCounter,
};
use nlmx_domain::ingestion::{
    Block, BlockKind, ChunkDraft, ChunkPolicy, DocumentId, DocumentStatus, DocumentSummary,
    PageBox, PageLayout, RemovalImpact, SourceDetails, StructuredDocument,
};
use nlmx_domain::parsed::{DocumentChunk, SectionKind};

/// Files held in memory; the "hash" is derived from the content so equal bytes ⇒ equal hash.
#[derive(Default)]
pub struct FakeFileStore {
    files: Mutex<HashMap<std::path::PathBuf, Vec<u8>>>,
    stored: Mutex<Vec<std::path::PathBuf>>,
    removed: Mutex<Vec<String>>,
    pruned_keeping: Mutex<Option<Vec<String>>>,
    /// Makes `remove` fail (e.g. a permission problem).
    pub fail_remove: std::sync::atomic::AtomicBool,
}

impl FakeFileStore {
    pub fn add(&self, path: impl Into<std::path::PathBuf>, content: &[u8]) -> String {
        let path = path.into();
        self.files.lock().unwrap().insert(path, content.to_vec());
        Self::hash(content)
    }

    pub fn library_path(sha256: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(format!("/library/{sha256}.pdf"))
    }

    /// How many times `store` copied a file.
    pub fn store_calls(&self) -> usize {
        self.stored.lock().unwrap().len()
    }

    /// Hashes whose library copy `remove` deleted, in order.
    pub fn removed(&self) -> Vec<String> {
        self.removed.lock().unwrap().clone()
    }

    /// The hashes passed to the last `prune`.
    pub fn pruned_keeping(&self) -> Option<Vec<String>> {
        self.pruned_keeping.lock().unwrap().clone()
    }

    fn hash(content: &[u8]) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:064x}", hasher.finish())
    }
}

impl FileStore for FakeFileStore {
    fn digest_text(&self, text: &str) -> FileDigest {
        FileDigest {
            sha256: Self::hash(format!("nlmx-note\0{text}").as_bytes()),
            size: text.len() as u64,
        }
    }

    fn digest<'a>(
        &'a self,
        path: &'a std::path::Path,
    ) -> BoxFuture<'a, Result<FileDigest, StorageError>> {
        Box::pin(async move {
            let files = self.files.lock().unwrap();
            let content = files
                .get(path)
                .ok_or_else(|| StorageError::new(format!("{} não existe", path.display())))?;
            Ok(FileDigest {
                sha256: Self::hash(content),
                size: content.len() as u64,
            })
        })
    }

    fn store<'a>(
        &'a self,
        _path: &'a std::path::Path,
        sha256: &'a str,
    ) -> BoxFuture<'a, Result<std::path::PathBuf, StorageError>> {
        Box::pin(async move {
            let path = Self::library_path(sha256);
            self.stored.lock().unwrap().push(path.clone());
            Ok(path)
        })
    }

    fn remove<'a>(&'a self, sha256: &'a str) -> BoxFuture<'a, Result<(), StorageError>> {
        Box::pin(async move {
            if self.fail_remove.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(StorageError::new("permissão negada"));
            }
            self.removed.lock().unwrap().push(sha256.to_string());
            Ok(())
        })
    }

    fn prune(&self, keep: Vec<String>) -> BoxFuture<'_, Result<u32, StorageError>> {
        Box::pin(async move {
            *self.pruned_keeping.lock().unwrap() = Some(keep);
            Ok(0)
        })
    }
}

#[derive(Debug, Clone)]
pub struct FakeDocumentRow {
    pub new: NewDocument,
    pub status: DocumentStatus,
    pub error: Option<String>,
    pub extraction: Option<Extraction>,
    /// What `save_processed` stored (any format); replaces `extraction` and the other way round.
    pub processed: Option<StoredExtraction>,
    /// Every status the document went through, in order.
    pub history: Vec<DocumentStatus>,
    /// Removed rows keep their slot so ids are never reused by the fake.
    pub removed: bool,
}

/// In-memory document repository that records status history.
#[derive(Default)]
pub struct FakeDocumentRepository {
    rows: Mutex<Vec<FakeDocumentRow>>,
}

impl FakeDocumentRepository {
    pub fn row(&self, id: DocumentId) -> FakeDocumentRow {
        self.rows.lock().unwrap()[id as usize - 1].clone()
    }

    pub fn len(&self) -> usize {
        self.rows.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Forces a status, e.g. to simulate an interrupted ingestion.
    pub fn force_status(&self, id: DocumentId, status: DocumentStatus) {
        self.rows.lock().unwrap()[id as usize - 1].status = status;
    }
}

impl DocumentRepository for FakeDocumentRepository {
    fn find_by_sha256<'a>(
        &'a self,
        sha256: &'a str,
    ) -> BoxFuture<'a, Result<Option<DocumentId>, StorageError>> {
        Box::pin(async move {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .position(|r| !r.removed && r.new.sha256 == sha256)
                .map(|i| i as DocumentId + 1))
        })
    }

    fn find_by_original_path<'a>(
        &'a self,
        path: &'a str,
    ) -> BoxFuture<'a, Result<Option<DocumentId>, StorageError>> {
        Box::pin(async move {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .rposition(|r| !r.removed && r.new.original_path == path)
                .map(|i| i as DocumentId + 1))
        })
    }

    fn replace_source(
        &self,
        id: DocumentId,
        source: NewDocument,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        Box::pin(async move {
            let mut rows = self.rows.lock().unwrap();
            let row = rows
                .get_mut(id as usize - 1)
                .filter(|r| !r.removed)
                .ok_or_else(|| StorageError::new("documento inexistente"))?;
            row.new = source;
            row.status = DocumentStatus::Queued;
            row.error = None;
            row.history.push(DocumentStatus::Queued);
            Ok(())
        })
    }

    fn insert(&self, document: NewDocument) -> BoxFuture<'_, Result<InsertOutcome, StorageError>> {
        Box::pin(async move {
            let mut rows = self.rows.lock().unwrap();
            if let Some(i) = rows
                .iter()
                .position(|r| !r.removed && r.new.sha256 == document.sha256)
            {
                return Ok(InsertOutcome::AlreadyExists(i as DocumentId + 1));
            }
            rows.push(FakeDocumentRow {
                new: document,
                status: DocumentStatus::Queued,
                error: None,
                extraction: None,
                processed: None,
                history: vec![DocumentStatus::Queued],
                removed: false,
            });
            Ok(InsertOutcome::Inserted(rows.len() as DocumentId))
        })
    }

    fn get(&self, id: DocumentId) -> BoxFuture<'_, Result<Option<DocumentRecord>, StorageError>> {
        Box::pin(async move {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .get(id as usize - 1)
                .filter(|r| !r.removed)
                .map(|r| DocumentRecord {
                    id,
                    sha256: r.new.sha256.clone(),
                    original_filename: r.new.original_filename.clone(),
                    library_path: r.new.library_path.clone(),
                    status: r.status,
                    file_size: r.new.file_size,
                    document_type: r.new.document_type,
                    mime_type: r.new.document_type.mime_types()[0].to_string(),
                    note_text: r.new.note_text.clone(),
                }))
        })
    }

    fn set_status(
        &self,
        id: DocumentId,
        status: DocumentStatus,
        error: Option<String>,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        Box::pin(async move {
            let mut rows = self.rows.lock().unwrap();
            let row = rows
                .get_mut(id as usize - 1)
                .filter(|r| !r.removed)
                .ok_or_else(|| StorageError::new("documento inexistente"))?;
            row.status = status;
            row.error = error;
            row.history.push(status);
            Ok(())
        })
    }

    fn save_extraction(
        &self,
        id: DocumentId,
        extraction: Extraction,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        Box::pin(async move {
            let mut rows = self.rows.lock().unwrap();
            let row = rows
                .get_mut(id as usize - 1)
                .filter(|r| !r.removed)
                .ok_or_else(|| StorageError::new("documento inexistente"))?;
            row.status = extraction.status;
            row.error = None;
            row.history.push(extraction.status);
            row.processed = None;
            row.extraction = Some(extraction);
            Ok(())
        })
    }

    fn save_processed(
        &self,
        id: DocumentId,
        extraction: StoredExtraction,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        Box::pin(async move {
            let mut rows = self.rows.lock().unwrap();
            let row = rows
                .get_mut(id as usize - 1)
                .filter(|r| !r.removed)
                .ok_or_else(|| StorageError::new("documento inexistente"))?;
            let kind = row.new.document_type;
            if !extraction.pages.is_empty() && !kind.is_paged() {
                return Err(StorageError::new(
                    "um documento sem páginas não tem páginas",
                ));
            }
            if extraction
                .chunks
                .iter()
                .any(|c| c.location.document_type() != kind || c.metadata.document_type != kind)
            {
                return Err(StorageError::new(
                    "um trecho de outro formato não cabe no documento",
                ));
            }
            row.status = extraction.status;
            row.error = None;
            row.history.push(extraction.status);
            row.extraction = None;
            row.processed = Some(extraction);
            Ok(())
        })
    }

    fn chunks_of(&self, id: DocumentId) -> BoxFuture<'_, Result<Vec<DocumentChunk>, StorageError>> {
        Box::pin(async move {
            let rows = self.rows.lock().unwrap();
            let Some(processed) = rows
                .get(id as usize - 1)
                .filter(|r| !r.removed)
                .and_then(|r| r.processed.as_ref())
            else {
                return Ok(Vec::new());
            };
            Ok(processed
                .chunks
                .iter()
                .map(|c| DocumentChunk {
                    chunk_id: Some(id * 100_000 + i64::from(c.index) + 1),
                    ..c.clone()
                })
                .collect())
        })
    }

    fn sections_of(
        &self,
        id: DocumentId,
    ) -> BoxFuture<'_, Result<Vec<SectionRecord>, StorageError>> {
        Box::pin(async move {
            let rows = self.rows.lock().unwrap();
            let Some(processed) = rows
                .get(id as usize - 1)
                .filter(|r| !r.removed)
                .and_then(|r| r.processed.as_ref())
            else {
                return Ok(Vec::new());
            };
            let mut records: Vec<SectionRecord> = Vec::new();
            for (ordinal, section) in processed.outline.iter().enumerate() {
                let parent_path: &[String] = match section.kind {
                    SectionKind::Heading => section.path.split_last().map_or(&[], |(_, r)| r),
                    SectionKind::Body => &section.path,
                };
                let parent_id = (!parent_path.is_empty())
                    .then(|| {
                        records
                            .iter()
                            .rev()
                            .find(|r| r.path == parent_path)
                            .map(|r| r.id)
                    })
                    .flatten();
                records.push(SectionRecord {
                    id: ordinal as i64 + 1,
                    parent_id,
                    kind: section.kind,
                    title: section.title.clone(),
                    level: section.level,
                    ordinal: ordinal as u32,
                    path: section.path.clone(),
                    location: section.location.clone(),
                });
            }
            Ok(records)
        })
    }

    fn source_details(
        &self,
        id: DocumentId,
    ) -> BoxFuture<'_, Result<Option<SourceDetails>, StorageError>> {
        Box::pin(async move {
            let summary = self.list().await?.into_iter().find(|d| d.id == id);
            let size = self
                .rows
                .lock()
                .unwrap()
                .get(id as usize - 1)
                .map_or(0, |r| r.new.file_size);
            Ok(summary.map(|d| SourceDetails {
                id: d.id,
                title: d.title,
                file_name: d.original_filename,
                document_type: d.document_type,
                status: d.status,
                error: d.error,
                chunks: d.chunk_count,
                file_size: size,
                page_count: d.page_count,
                sheets: None,
                imported_at: d.imported_at,
                indexed_at: None,
                conversations: 0,
            }))
        })
    }

    fn list(&self) -> BoxFuture<'_, Result<Vec<DocumentSummary>, StorageError>> {
        Box::pin(async move {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                .filter(|(_, r)| !r.removed)
                .map(|(i, r)| DocumentSummary {
                    id: i as DocumentId + 1,
                    title: r
                        .extraction
                        .as_ref()
                        .map(|e| e.title.clone())
                        .or_else(|| r.processed.as_ref().map(|e| e.title.clone()))
                        .unwrap_or_else(|| r.new.original_filename.clone()),
                    original_filename: r.new.original_filename.clone(),
                    document_type: r.new.document_type,
                    page_count: r.extraction.as_ref().map(|e| e.page_count).or_else(|| {
                        r.processed
                            .as_ref()
                            .filter(|_| r.new.document_type.is_paged())
                            .map(|e| e.pages.len() as u32)
                    }),
                    chunk_count: r.extraction.as_ref().map_or_else(
                        || r.processed.as_ref().map_or(0, |e| e.chunks.len() as u32),
                        |e| e.chunks.len() as u32,
                    ),
                    status: r.status,
                    error: r.error.clone(),
                    imported_at: "2026-01-01T00:00:00.000Z".into(),
                })
                .collect())
        })
    }

    fn unfinished(&self) -> BoxFuture<'_, Result<Vec<DocumentId>, StorageError>> {
        Box::pin(async move {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                .filter(|(_, r)| !r.removed && r.status.is_unfinished())
                .map(|(i, _)| i as DocumentId + 1)
                .collect())
        })
    }

    fn pages(
        &self,
        id: DocumentId,
    ) -> BoxFuture<'_, Result<Vec<nlmx_application::ports::PageRecord>, StorageError>> {
        Box::pin(async move {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .get(id as usize - 1)
                .filter(|r| !r.removed)
                .map(|r| match (&r.extraction, &r.processed) {
                    (Some(e), _) => e.pages.clone(),
                    (None, Some(p)) => p.pages.clone(),
                    _ => Vec::new(),
                })
                .unwrap_or_default())
        })
    }

    /// The fake has no chat history: only chunks count.
    fn removal_impact(&self, id: DocumentId) -> BoxFuture<'_, Result<RemovalImpact, StorageError>> {
        Box::pin(async move {
            let rows = self.rows.lock().unwrap();
            let chunks = rows
                .get(id as usize - 1)
                .filter(|r| !r.removed)
                .map_or(0, |r| match (&r.extraction, &r.processed) {
                    (Some(e), _) => e.chunks.len() as u32,
                    (None, Some(p)) => p.chunks.len() as u32,
                    _ => 0,
                });
            Ok(RemovalImpact {
                chunks,
                ..RemovalImpact::default()
            })
        })
    }

    fn remove(
        &self,
        id: DocumentId,
    ) -> BoxFuture<'_, Result<Option<RemovedDocument>, StorageError>> {
        Box::pin(async move {
            let impact = self.removal_impact(id).await?;
            let mut rows = self.rows.lock().unwrap();
            Ok(rows
                .get_mut(id as usize - 1)
                .filter(|r| !r.removed)
                .map(|row| {
                    row.removed = true;
                    RemovedDocument {
                        sha256: row.new.sha256.clone(),
                        impact,
                    }
                }))
        })
    }

    fn hashes(&self) -> BoxFuture<'_, Result<Vec<String>, StorageError>> {
        Box::pin(async move {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|r| !r.removed)
                .map(|r| r.new.sha256.clone())
                .collect())
        })
    }
}

/// The fake has no vectors: `model` is always `None`, and there are no timestamps.
impl nlmx_application::ports::IndexingReader for FakeDocumentRepository {
    fn snapshot(
        &self,
        _model_id: Option<String>,
    ) -> BoxFuture<'_, Result<nlmx_domain::indexing::IndexSnapshot, StorageError>> {
        Box::pin(async move {
            let rows = self.rows.lock().unwrap();
            let jobs: Vec<nlmx_domain::indexing::IndexJob> = rows
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, r)| !r.removed)
                .map(|(i, r)| nlmx_domain::indexing::IndexJob {
                    document_id: i as DocumentId + 1,
                    document_type: r.new.document_type,
                    title: r
                        .extraction
                        .as_ref()
                        .map(|e| e.title.clone())
                        .unwrap_or_else(|| r.new.original_filename.clone()),
                    status: r.status,
                    error: r.error.clone(),
                    chunks: r.extraction.as_ref().map_or(0, |e| e.chunks.len() as u32),
                    attempts: r
                        .history
                        .iter()
                        .filter(|s| **s == DocumentStatus::Extracting)
                        .count() as u32,
                    started_at: None,
                    finished_at: None,
                    duration_ms: None,
                })
                .collect();
            Ok(nlmx_domain::indexing::IndexSnapshot {
                chunks: jobs.iter().map(|j| j.chunks).sum(),
                jobs,
                model: None,
            })
        })
    }
}

/// Every span becomes a paragraph, outside any section.
pub struct FakeStructureAnalyzer;

impl StructureAnalyzer for FakeStructureAnalyzer {
    fn version(&self) -> u32 {
        99
    }

    fn analyze(&self, pages: &[PageLayout]) -> StructuredDocument {
        let blocks = pages
            .iter()
            .flat_map(|page| {
                page.spans.iter().map(|span| Block {
                    kind: BlockKind::Paragraph,
                    text: span.text.clone(),
                    page: page.number,
                    boxes: vec![PageBox {
                        page: page.number,
                        bbox: span.bbox,
                    }],
                    section_path: vec![],
                })
            })
            .collect();
        StructuredDocument { blocks }
    }
}

/// Every block becomes a chunk; the "hash" is the text itself.
pub struct FakeChunker;

impl Chunker for FakeChunker {
    fn version(&self) -> u32 {
        98
    }

    fn chunk(
        &self,
        document: &StructuredDocument,
        _policy: &ChunkPolicy,
        tokens: &dyn TokenCounter,
    ) -> Vec<ChunkDraft> {
        document
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| ChunkDraft {
                index: i as u32,
                text: b.text.clone(),
                token_count: tokens.count(&b.text),
                page_start: b.page,
                page_end: b.page,
                section_path: b.section_path.clone(),
                boxes: b.boxes.clone(),
                content_hash: b.text.clone(),
            })
            .collect()
    }
}

/// One token per word.
pub struct WordTokenCounter;

impl TokenCounter for WordTokenCounter {
    fn count(&self, text: &str) -> u32 {
        text.split_whitespace().count() as u32
    }
}

// ── Embedding fake ───────────────────────────────────────────────────────────

use nlmx_application::ports::EmbeddingProvider;
use nlmx_domain::embedding::{EmbeddingError, EmbeddingPurpose, ModelIdentity};

/// Deterministic bag-of-words vectors: texts sharing words get similar (normalized) vectors.
pub struct FakeEmbeddingProvider {
    pub dimensions: u32,
}

impl Default for FakeEmbeddingProvider {
    fn default() -> Self {
        Self { dimensions: 32 }
    }
}

impl FakeEmbeddingProvider {
    fn vector(&self, text: &str) -> Vec<f32> {
        use std::hash::{Hash, Hasher};
        let mut v = vec![0f32; self.dimensions as usize];
        for word in text.split_whitespace().map(|w| w.to_lowercase()) {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            word.hash(&mut h);
            v[(h.finish() % self.dimensions as u64) as usize] += 1.0;
        }
        let norm = v
            .iter()
            .map(|x| x * x)
            .sum::<f32>()
            .sqrt()
            .max(f32::EPSILON);
        v.iter_mut().for_each(|x| *x /= norm);
        v
    }
}

impl EmbeddingProvider for FakeEmbeddingProvider {
    fn model_id(&self) -> &str {
        "fake-embedding"
    }

    fn identity(&self) -> BoxFuture<'_, Result<ModelIdentity, EmbeddingError>> {
        let identity = ModelIdentity {
            id: "fake-embedding".into(),
            file_name: "fake.gguf".into(),
            file_size: 0,
            dimensions: self.dimensions,
            context_length: 512,
            parameters: 0,
        };
        Box::pin(async move { Ok(identity) })
    }

    fn dimensions(&self) -> BoxFuture<'_, Result<u32, EmbeddingError>> {
        let d = self.dimensions;
        Box::pin(async move { Ok(d) })
    }

    fn embed<'a>(
        &'a self,
        text: &'a str,
        _purpose: EmbeddingPurpose,
    ) -> BoxFuture<'a, Result<Vec<f32>, EmbeddingError>> {
        Box::pin(async move { Ok(self.vector(text)) })
    }

    fn embed_batch<'a>(
        &'a self,
        texts: &'a [String],
        _purpose: EmbeddingPurpose,
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, EmbeddingError>> {
        Box::pin(async move { Ok(texts.iter().map(|t| self.vector(t)).collect()) })
    }
}

// ── Model manager fake ───────────────────────────────────────────────────────

use nlmx_application::ports::{ModelProvider, ProgressCallback};
use nlmx_domain::models::{
    ConfirmedDownload, DiskSpace, DownloadPlan, DownloadProgress, InstalledModel, ModelDescriptor,
    ModelError, ModelState, VerifyReport,
};

/// In-memory model manager: downloads complete instantly (reporting progress) once confirmed.
pub struct FakeModelProvider {
    catalog: Vec<ModelDescriptor>,
    installed: Mutex<Vec<InstalledModel>>,
    active: Mutex<Option<String>>,
    pub available_bytes: u64,
}

impl FakeModelProvider {
    pub fn new(catalog: Vec<ModelDescriptor>) -> Self {
        Self {
            catalog,
            installed: Mutex::default(),
            active: Mutex::default(),
            available_bytes: u64::MAX / 2,
        }
    }

    fn find(&self, id: &str) -> Result<&ModelDescriptor, ModelError> {
        self.catalog
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| ModelError::UnknownModel(id.into()))
    }

    fn installed_model(&self, id: &str) -> Option<InstalledModel> {
        self.installed
            .lock()
            .unwrap()
            .iter()
            .find(|m| m.id == id)
            .cloned()
    }

    fn plan_for(&self, model: &ModelDescriptor, replaces: Option<String>) -> DownloadPlan {
        DownloadPlan {
            model: model.clone(),
            resume_from: 0,
            required_bytes: model.size,
            available_bytes: self.available_bytes,
            replaces,
        }
    }
}

impl ModelProvider for FakeModelProvider {
    fn catalog(&self) -> Vec<ModelDescriptor> {
        self.catalog.clone()
    }

    fn installed(&self) -> BoxFuture<'_, Result<Vec<InstalledModel>, ModelError>> {
        Box::pin(async move { Ok(self.installed.lock().unwrap().clone()) })
    }

    fn status<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<ModelState, ModelError>> {
        Box::pin(async move {
            let model = self.find(id)?;
            Ok(match self.installed_model(id) {
                None => ModelState::NotInstalled,
                Some(m) if m.version == model.version => {
                    ModelState::Installed { version: m.version }
                }
                Some(m) => ModelState::UpdateAvailable {
                    installed: m.version,
                    latest: model.version.clone(),
                },
            })
        })
    }

    fn plan_download<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<DownloadPlan, ModelError>> {
        Box::pin(async move {
            let model = self.find(id)?;
            if self
                .installed_model(id)
                .is_some_and(|m| m.version == model.version)
            {
                return Err(ModelError::AlreadyInstalled(id.into()));
            }
            Ok(self.plan_for(model, self.installed_model(id).map(|m| m.version)))
        })
    }

    fn download(
        &self,
        confirmed: ConfirmedDownload,
        progress: ProgressCallback,
        cancel: CancelFlag,
    ) -> BoxFuture<'_, Result<InstalledModel, ModelError>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(ModelError::Cancelled);
            }
            let model = confirmed.plan().model.clone();
            progress(DownloadProgress {
                received: model.size,
                total: model.size,
                bytes_per_second: 0.0,
            });
            let installed = InstalledModel {
                id: model.id.clone(),
                version: model.version.clone(),
                path: format!("/models/{}/{}/{}", model.id, model.version, model.file_name),
                size: model.size,
                sha256: model.sha256.clone(),
                installed_at: "2026-01-01T00:00:00Z".into(),
                verified_at: None,
            };
            let mut all = self.installed.lock().unwrap();
            all.retain(|m| m.id != model.id);
            all.push(installed.clone());
            Ok(installed)
        })
    }

    fn verify<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<VerifyReport, ModelError>> {
        Box::pin(async move {
            let model = self
                .installed_model(id)
                .ok_or_else(|| ModelError::NotInstalled(id.into()))?;
            Ok(VerifyReport {
                actual_sha256: model.sha256.clone(),
                model,
                ok: true,
            })
        })
    }

    fn remove<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), ModelError>> {
        Box::pin(async move {
            self.installed.lock().unwrap().retain(|m| m.id != id);
            let mut active = self.active.lock().unwrap();
            if active.as_deref() == Some(id) {
                *active = None;
            }
            Ok(())
        })
    }

    fn plan_update<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<DownloadPlan, ModelError>> {
        Box::pin(async move {
            let model = self.find(id)?;
            let installed = self
                .installed_model(id)
                .ok_or_else(|| ModelError::NotInstalled(id.into()))?;
            if installed.version == model.version {
                return Err(ModelError::AlreadyInstalled(id.into()));
            }
            Ok(self.plan_for(model, Some(installed.version)))
        })
    }

    fn activate<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<InstalledModel, ModelError>> {
        Box::pin(async move {
            let model = self
                .installed_model(id)
                .ok_or_else(|| ModelError::NotInstalled(id.into()))?;
            *self.active.lock().unwrap() = Some(id.into());
            Ok(model)
        })
    }

    fn active(&self) -> BoxFuture<'_, Result<Option<InstalledModel>, ModelError>> {
        Box::pin(async move {
            Ok(self
                .active
                .lock()
                .unwrap()
                .clone()
                .and_then(|id| self.installed_model(&id)))
        })
    }

    fn disk(&self) -> BoxFuture<'_, Result<DiskSpace, ModelError>> {
        Box::pin(async move {
            let used = self.installed.lock().unwrap().iter().map(|m| m.size).sum();
            Ok(DiskSpace {
                models_bytes: used,
                available_bytes: self.available_bytes,
            })
        })
    }
}

/// A runtime with fixed information.
pub struct FakeRuntime(pub Result<nlmx_domain::models::RuntimeInfo, String>);

impl FakeRuntime {
    pub fn llama() -> Self {
        Self(Ok(nlmx_domain::models::RuntimeInfo {
            name: "llama.cpp".into(),
            build: "b11349".into(),
            commit: Some("fb4b2737a".into()),
            path: "/Applications/NLMX.app/Contents/Resources/llama/llama-server".into(),
            bundled: true,
        }))
    }
}

impl nlmx_application::ports::InferenceRuntime for FakeRuntime {
    fn info(&self) -> BoxFuture<'_, Result<nlmx_domain::models::RuntimeInfo, String>> {
        let info = self.0.clone();
        Box::pin(async move { info })
    }
}

// ── Vector store fake ────────────────────────────────────────────────────────

use nlmx_application::ports::VectorStore;
use nlmx_domain::vectors::{
    ChunkId, DeleteScope, EmbeddingSpace, VectorError, VectorFilter, VectorHit, VectorIndex,
    VectorIndexId,
};

/// Brute-force in-memory vector store. Chunks must be registered with their document first.
#[derive(Default)]
pub struct FakeVectorStore {
    indexes: Mutex<Vec<EmbeddingSpace>>,
    vectors: Mutex<HashMap<(VectorIndexId, ChunkId), Vec<f32>>>,
    documents: Mutex<HashMap<ChunkId, DocumentId>>,
}

impl FakeVectorStore {
    pub fn with_chunk(self, chunk: ChunkId, document: DocumentId) -> Self {
        self.documents.lock().unwrap().insert(chunk, document);
        self
    }

    fn dims(&self, index: VectorIndexId) -> Result<u32, VectorError> {
        let indexes = self.indexes.lock().unwrap();
        indexes
            .get((index - 1) as usize)
            .map(|s| s.dimensions)
            .ok_or(VectorError::UnknownIndex(index))
    }
}

fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    1.0 - dot / (norm(a) * norm(b)).max(f32::EPSILON)
}

impl VectorStore for FakeVectorStore {
    fn create_index<'a>(
        &'a self,
        space: &'a EmbeddingSpace,
    ) -> BoxFuture<'a, Result<VectorIndex, VectorError>> {
        Box::pin(async move {
            let mut indexes = self.indexes.lock().unwrap();
            let position = indexes
                .iter()
                .position(|s| s.model_id == space.model_id && s.revision == space.revision);
            let id = match position {
                Some(i) if indexes[i].dimensions != space.dimensions => {
                    return Err(VectorError::DimensionMismatch {
                        expected: indexes[i].dimensions,
                        actual: space.dimensions,
                    });
                }
                Some(i) => i as VectorIndexId + 1,
                None => {
                    indexes.push(space.clone());
                    indexes.len() as VectorIndexId
                }
            };
            let count = self
                .vectors
                .lock()
                .unwrap()
                .keys()
                .filter(|(i, _)| *i == id)
                .count() as u64;
            Ok(VectorIndex {
                id,
                space: space.clone(),
                table: format!("fake_{id}"),
                count,
            })
        })
    }

    fn insert_embedding<'a>(
        &'a self,
        index: VectorIndexId,
        chunk: ChunkId,
        vector: &'a [f32],
    ) -> BoxFuture<'a, Result<(), VectorError>> {
        Box::pin(async move {
            let dims = self.dims(index)?;
            if vector.len() != dims as usize {
                return Err(VectorError::DimensionMismatch {
                    expected: dims,
                    actual: vector.len() as u32,
                });
            }
            if !self.documents.lock().unwrap().contains_key(&chunk) {
                return Err(VectorError::UnknownChunk(chunk));
            }
            self.vectors
                .lock()
                .unwrap()
                .insert((index, chunk), vector.to_vec());
            Ok(())
        })
    }

    fn insert_batch<'a>(
        &'a self,
        index: VectorIndexId,
        items: &'a [(ChunkId, Vec<f32>)],
    ) -> BoxFuture<'a, Result<(), VectorError>> {
        Box::pin(async move {
            let dims = self.dims(index)?;
            for (chunk, v) in items {
                if v.len() != dims as usize {
                    return Err(VectorError::DimensionMismatch {
                        expected: dims,
                        actual: v.len() as u32,
                    });
                }
                if !self.documents.lock().unwrap().contains_key(chunk) {
                    return Err(VectorError::UnknownChunk(*chunk));
                }
            }
            let mut vectors = self.vectors.lock().unwrap();
            for (chunk, v) in items {
                vectors.insert((index, *chunk), v.clone());
            }
            Ok(())
        })
    }

    fn search<'a>(
        &'a self,
        index: VectorIndexId,
        query: &'a [f32],
        k: usize,
        filter: &'a VectorFilter,
    ) -> BoxFuture<'a, Result<Vec<VectorHit>, VectorError>> {
        Box::pin(async move {
            let dims = self.dims(index)?;
            if query.len() != dims as usize {
                return Err(VectorError::DimensionMismatch {
                    expected: dims,
                    actual: query.len() as u32,
                });
            }
            let documents = self.documents.lock().unwrap();
            let mut hits: Vec<VectorHit> = self
                .vectors
                .lock()
                .unwrap()
                .iter()
                .filter(|((i, _), _)| *i == index)
                .map(|((_, chunk), v)| {
                    let distance = cosine_distance(query, v);
                    VectorHit {
                        chunk_id: *chunk,
                        document_id: documents[chunk],
                        distance,
                        similarity: 1.0 - distance,
                    }
                })
                .filter(|h| {
                    filter
                        .documents
                        .as_ref()
                        .is_none_or(|d| d.contains(&h.document_id))
                })
                .filter(|h| {
                    filter
                        .chunks
                        .as_ref()
                        .is_none_or(|c| c.contains(&h.chunk_id))
                })
                .collect();
            hits.sort_by(|a, b| {
                a.distance
                    .total_cmp(&b.distance)
                    .then(a.chunk_id.cmp(&b.chunk_id))
            });
            hits.truncate(k.max(1));
            Ok(hits)
        })
    }

    fn delete<'a>(
        &'a self,
        index: VectorIndexId,
        scope: &'a DeleteScope,
    ) -> BoxFuture<'a, Result<u64, VectorError>> {
        Box::pin(async move {
            self.dims(index)?;
            let documents = self.documents.lock().unwrap();
            let mut vectors = self.vectors.lock().unwrap();
            let before = vectors.len();
            vectors.retain(|(i, chunk), _| {
                *i != index
                    || match scope {
                        DeleteScope::Chunks(ids) => !ids.contains(chunk),
                        DeleteScope::Document(d) => documents.get(chunk) != Some(d),
                        DeleteScope::All => false,
                    }
            });
            Ok((before - vectors.len()) as u64)
        })
    }

    fn rebuild(&self, index: VectorIndexId) -> BoxFuture<'_, Result<u64, VectorError>> {
        Box::pin(async move { self.count(index).await })
    }

    fn count(&self, index: VectorIndexId) -> BoxFuture<'_, Result<u64, VectorError>> {
        Box::pin(async move {
            self.dims(index)?;
            Ok(self
                .vectors
                .lock()
                .unwrap()
                .keys()
                .filter(|(i, _)| *i == index)
                .count() as u64)
        })
    }
}

// ── Lexical index and chunk reader fake ──────────────────────────────────────

use nlmx_application::ports::{Candidates, ChunkReader, ChunkView, LexicalIndex};
use nlmx_domain::retrieval::{LexicalCandidate, LexicalQuery, RetrievalFilter};

/// Chunks in memory with a naive lexical score (term occurrences, accent-sensitive).
use nlmx_domain::{
    document_type::DocumentType,
    source::{SourceLocation, SourceReference},
};

fn fake_chunk_metadata(
    document_type: DocumentType,
    document_id: DocumentId,
) -> nlmx_domain::parsed::ChunkMetadata {
    nlmx_domain::parsed::ChunkMetadata {
        document_type,
        document_title: Some(format!("Documento {document_id}")),
        file_name: None,
        language: None,
        columns: Vec::new(),
    }
}

#[derive(Default)]
pub struct FakeCorpus {
    chunks: Vec<ChunkView>,
    collections: HashMap<i64, Vec<DocumentId>>,
}

impl FakeCorpus {
    pub fn chunk(mut self, chunk_id: i64, document_id: DocumentId, page: u32, text: &str) -> Self {
        self.chunks.push(ChunkView {
            chunk_id,
            document_id,
            document_title: format!("Documento {document_id}"),
            document_name: format!("documento-{document_id}.pdf"),
            document_type: DocumentType::Pdf,
            ordinal: chunk_id as u32,
            content_hash: format!("hash-{chunk_id}"),
            page_start: page,
            page_end: page,
            section: None,
            text: text.into(),
            bboxes: vec![],
            location: SourceLocation::Pdf {
                page_start: page,
                page_end: page,
                boxes: vec![],
            },
            metadata: fake_chunk_metadata(DocumentType::Pdf, document_id),
        });
        self
    }

    /// A chunk of a document that is not a PDF, at `location` (its format is the location's).
    pub fn located(
        mut self,
        chunk_id: i64,
        document_id: DocumentId,
        document_name: &str,
        location: SourceLocation,
        text: &str,
    ) -> Self {
        let document_type = location.document_type();
        self.chunks.push(ChunkView {
            chunk_id,
            document_id,
            document_title: format!("Documento {document_id}"),
            document_name: document_name.to_string(),
            document_type,
            ordinal: chunk_id as u32,
            content_hash: format!("hash-{chunk_id}"),
            page_start: 1,
            page_end: 1,
            section: None,
            text: text.into(),
            bboxes: vec![],
            location,
            metadata: fake_chunk_metadata(document_type, document_id),
        });
        self
    }

    /// Sets the section of the most recently added chunk.
    pub fn in_section(mut self, section: &str) -> Self {
        if let Some(last) = self.chunks.last_mut() {
            last.section = Some(section.to_string());
        }
        self
    }

    /// Sets the title of every chunk of `document`.
    pub fn titled(mut self, document: DocumentId, title: &str) -> Self {
        for c in self.chunks.iter_mut().filter(|c| c.document_id == document) {
            c.document_title = title.to_string();
        }
        self
    }

    pub fn collection(mut self, collection: i64, documents: Vec<DocumentId>) -> Self {
        self.collections.insert(collection, documents);
        self
    }

    fn allowed(&self, chunk: &ChunkView, filter: &RetrievalFilter) -> bool {
        filter
            .documents
            .as_ref()
            .is_none_or(|d| d.contains(&chunk.document_id))
            && filter.collections.as_ref().is_none_or(|cs| {
                cs.iter().any(|c| {
                    self.collections
                        .get(c)
                        .is_some_and(|docs| docs.contains(&chunk.document_id))
                })
            })
            && filter
                .pages
                .is_none_or(|p| chunk.page_start <= p.to && chunk.page_end >= p.from)
    }
}

impl LexicalIndex for FakeCorpus {
    fn search<'a>(
        &'a self,
        query: &'a LexicalQuery,
        k: usize,
        filter: &'a RetrievalFilter,
    ) -> BoxFuture<'a, Result<Vec<LexicalCandidate>, StorageError>> {
        Box::pin(async move {
            let mut hits: Vec<LexicalCandidate> = self
                .chunks
                .iter()
                .filter(|c| self.allowed(c, filter))
                .filter_map(|c| {
                    let text = c.text.to_lowercase();
                    let score: usize = query
                        .terms
                        .iter()
                        .map(|t| text.matches(t.as_str()).count())
                        .sum();
                    (score > 0).then_some(LexicalCandidate {
                        chunk_id: c.chunk_id,
                        document_id: c.document_id,
                        bm25: -(score as f64),
                    })
                })
                .collect();
            hits.sort_by(|a, b| a.bm25.total_cmp(&b.bm25).then(a.chunk_id.cmp(&b.chunk_id)));
            hits.truncate(k);
            Ok(hits)
        })
    }
}

impl ChunkReader for FakeCorpus {
    fn resolve<'a>(
        &'a self,
        filter: &'a RetrievalFilter,
    ) -> BoxFuture<'a, Result<Candidates, StorageError>> {
        Box::pin(async move {
            let doc_filter = RetrievalFilter {
                pages: None,
                ..filter.clone()
            };
            let documents =
                (filter.documents.is_some() || filter.collections.is_some()).then(|| {
                    let mut docs: Vec<DocumentId> = self
                        .chunks
                        .iter()
                        .filter(|c| self.allowed(c, &doc_filter))
                        .map(|c| c.document_id)
                        .collect();
                    docs.sort();
                    docs.dedup();
                    docs
                });
            let chunks = filter.pages.map(|_| {
                self.chunks
                    .iter()
                    .filter(|c| self.allowed(c, filter))
                    .map(|c| c.chunk_id)
                    .collect()
            });
            Ok(Candidates { documents, chunks })
        })
    }

    fn document_chunks(
        &self,
        document: DocumentId,
    ) -> BoxFuture<'_, Result<Vec<ChunkView>, StorageError>> {
        Box::pin(async move {
            Ok(self
                .chunks
                .iter()
                .filter(|c| c.document_id == document)
                .cloned()
                .collect())
        })
    }

    fn get_many<'a>(
        &'a self,
        ids: &'a [i64],
    ) -> BoxFuture<'a, Result<Vec<ChunkView>, StorageError>> {
        Box::pin(async move {
            Ok(ids
                .iter()
                .filter_map(|id| self.chunks.iter().find(|c| c.chunk_id == *id).cloned())
                .collect())
        })
    }

    fn chunked_documents(&self) -> BoxFuture<'_, Result<Vec<DocumentId>, StorageError>> {
        Box::pin(async move {
            let mut docs: Vec<DocumentId> = self.chunks.iter().map(|c| c.document_id).collect();
            docs.sort();
            docs.dedup();
            Ok(docs)
        })
    }
}

/// An embedding source with a fixed (or no) model.
pub struct FixedEmbeddingSource(
    pub Option<std::sync::Arc<dyn nlmx_application::ports::EmbeddingProvider>>,
);

impl FixedEmbeddingSource {
    pub fn none() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self(None))
    }

    pub fn of(
        provider: impl nlmx_application::ports::EmbeddingProvider + 'static,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self(Some(std::sync::Arc::new(provider))))
    }

    /// A provider that is also used (and shut down) elsewhere.
    pub fn of_arc(
        provider: std::sync::Arc<dyn nlmx_application::ports::EmbeddingProvider>,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self(Some(provider)))
    }
}

impl nlmx_application::ports::EmbeddingSource for FixedEmbeddingSource {
    fn current(&self) -> Option<std::sync::Arc<dyn nlmx_application::ports::EmbeddingProvider>> {
        self.0.clone()
    }
}

/// What an `LlmProvider` under contract test must be set up to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmScenario {
    /// Available; answers [`CONTRACT_ANSWER`] in several pieces.
    Answer,
    /// Answers [`CONTRACT_ANSWER`] slowly enough to be cancelled after the first piece.
    Slow,
    /// The safety guardrails decline.
    Refuse,
}

pub const CONTRACT_ANSWER: &str = "A carência é de cento e oitenta dias [1].";

/// The contract every `LlmProvider` must honour; `make` builds one set up for each scenario.
pub async fn llm_provider_contract<P, F>(make: F)
where
    P: LlmProvider + ?Sized,
    F: Fn(LlmScenario) -> std::sync::Arc<P>,
{
    use std::sync::Arc;

    let request = |user: &str| GenerationRequest {
        system: "Responda só com base nos trechos.".into(),
        history: Vec::new(),
        user: user.into(),
        temperature: 0.2,
        max_tokens: 200,
    };

    let provider = make(LlmScenario::Answer);
    assert!(provider.status().await.is_available(), "status");
    assert!(
        provider.capabilities().context_tokens >= 1024,
        "context window"
    );
    let short = provider
        .count_tokens(&request("Qual a carência?"))
        .await
        .unwrap();
    let long = provider
        .count_tokens(&request(&"Qual a carência do plano? ".repeat(40)))
        .await
        .unwrap();
    assert!(
        short > 0 && long > short,
        "token counts grow: {short} → {long}"
    );

    let pieces = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = pieces.clone();
    let generation = provider
        .generate(
            &request("Qual a carência?"),
            &move |t: &str| sink.lock().unwrap().push(t.to_string()),
            CancelFlag::default(),
        )
        .await
        .unwrap();
    assert_eq!(generation.text, CONTRACT_ANSWER);
    assert_eq!(generation.finish, FinishReason::Completed);
    let streamed: String = pieces.lock().unwrap().concat();
    assert_eq!(
        streamed.trim_end(),
        CONTRACT_ANSWER,
        "pieces add up to the text"
    );
    assert!(pieces.lock().unwrap().len() > 1, "streamed in pieces");

    let provider = make(LlmScenario::Slow);
    let cancel = CancelFlag::default();
    let trigger = cancel.clone();
    let generation = provider
        .generate(
            &request("Qual a carência?"),
            &move |_| trigger.cancel(),
            cancel,
        )
        .await
        .unwrap();
    assert_eq!(generation.finish, FinishReason::Cancelled);
    assert!(
        generation.text.len() < CONTRACT_ANSWER.len(),
        "partial: {:?}",
        generation.text
    );

    let provider = make(LlmScenario::Refuse);
    assert!(matches!(
        provider
            .generate(&request("Qual a carência?"), &|_| {}, CancelFlag::default())
            .await,
        Err(LlmError::Refused(_))
    ));
}

#[cfg(test)]
mod llm_contract {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn the_fake_honours_the_contract() {
        let make = |scenario| {
            Arc::new(match scenario {
                LlmScenario::Answer | LlmScenario::Slow => {
                    FakeLlmProvider::available().answering(CONTRACT_ANSWER)
                }
                LlmScenario::Refuse => {
                    FakeLlmProvider::available().failing(LlmError::Refused("guardrails".into()))
                }
            })
        };
        block_on(llm_provider_contract(make));
    }

    /// Minimal executor: the fake never awaits anything that isn't immediately ready.
    pub(crate) fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Waker};
        let mut future = std::pin::pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
                return output;
            }
        }
    }
}

// ── Conversations ────────────────────────────────────────────────────────────

use nlmx_application::ports::ConversationRepository;
use nlmx_domain::chat::{
    AnswerGrounding, Conversation, ConversationId, ConversationScope, ConversationSummary, Message,
    MessageId, MessageSource, MessageStatus, Role,
};

/// In-memory conversations; timestamps are a counter (enough for ordering).
#[derive(Default)]
pub struct FakeConversations {
    state: Mutex<ConversationState>,
}

#[derive(Default)]
struct ConversationState {
    clock: u64,
    conversations: Vec<Conversation>,
    messages: Vec<Message>,
}

impl ConversationState {
    fn tick(&mut self) -> String {
        self.clock += 1;
        format!("{:020}", self.clock)
    }

    fn touch(&mut self, id: ConversationId) {
        let now = self.tick();
        if let Some(c) = self.conversations.iter_mut().find(|c| c.id == id) {
            c.updated_at = now;
        }
    }
}

impl ConversationRepository for FakeConversations {
    fn create(
        &self,
        scope: ConversationScope,
    ) -> BoxFuture<'_, Result<Conversation, StorageError>> {
        Box::pin(async move {
            let mut s = self.state.lock().unwrap();
            let updated_at = s.tick();
            let conversation = Conversation {
                id: s.conversations.iter().map(|c| c.id).max().unwrap_or(0) + 1,
                title: None,
                scope,
                updated_at,
            };
            s.conversations.push(conversation.clone());
            Ok(conversation)
        })
    }

    fn get(&self, id: ConversationId) -> BoxFuture<'_, Result<Option<Conversation>, StorageError>> {
        Box::pin(async move {
            Ok(self
                .state
                .lock()
                .unwrap()
                .conversations
                .iter()
                .find(|c| c.id == id)
                .cloned())
        })
    }

    fn recent(&self, limit: u32) -> BoxFuture<'_, Result<Vec<ConversationSummary>, StorageError>> {
        Box::pin(async move {
            let s = self.state.lock().unwrap();
            let mut list: Vec<ConversationSummary> = s
                .conversations
                .iter()
                .map(|c| ConversationSummary {
                    id: c.id,
                    title: c.title.clone(),
                    updated_at: c.updated_at.clone(),
                    messages: s
                        .messages
                        .iter()
                        .filter(|m| m.conversation_id == c.id)
                        .count() as u32,
                })
                .collect();
            list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(b.id.cmp(&a.id)));
            list.truncate(limit as usize);
            Ok(list)
        })
    }

    fn set_scope(
        &self,
        id: ConversationId,
        scope: ConversationScope,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        Box::pin(async move {
            let mut s = self.state.lock().unwrap();
            if let Some(c) = s.conversations.iter_mut().find(|c| c.id == id) {
                c.scope = scope;
            }
            s.touch(id);
            Ok(())
        })
    }

    fn set_title<'a>(
        &'a self,
        id: ConversationId,
        title: &'a str,
    ) -> BoxFuture<'a, Result<(), StorageError>> {
        Box::pin(async move {
            let mut s = self.state.lock().unwrap();
            if let Some(c) = s.conversations.iter_mut().find(|c| c.id == id) {
                c.title = Some(title.to_string());
            }
            Ok(())
        })
    }

    fn delete(&self, id: ConversationId) -> BoxFuture<'_, Result<(), StorageError>> {
        Box::pin(async move {
            let mut s = self.state.lock().unwrap();
            s.conversations.retain(|c| c.id != id);
            s.messages.retain(|m| m.conversation_id != id);
            Ok(())
        })
    }

    fn messages(&self, id: ConversationId) -> BoxFuture<'_, Result<Vec<Message>, StorageError>> {
        Box::pin(async move {
            Ok(self
                .state
                .lock()
                .unwrap()
                .messages
                .iter()
                .filter(|m| m.conversation_id == id)
                .cloned()
                .collect())
        })
    }

    fn message(&self, id: MessageId) -> BoxFuture<'_, Result<Option<Message>, StorageError>> {
        Box::pin(async move {
            Ok(self
                .state
                .lock()
                .unwrap()
                .messages
                .iter()
                .find(|m| m.id == id)
                .cloned())
        })
    }

    fn add_message<'a>(
        &'a self,
        conversation: ConversationId,
        role: Role,
        content: &'a str,
        status: MessageStatus,
        grounding: Option<AnswerGrounding>,
    ) -> BoxFuture<'a, Result<MessageId, StorageError>> {
        Box::pin(async move {
            let mut s = self.state.lock().unwrap();
            let id = s.messages.iter().map(|m| m.id).max().unwrap_or(0) + 1;
            let created_at = s.tick();
            s.messages.push(Message {
                id,
                conversation_id: conversation,
                role,
                content: content.to_string(),
                status,
                grounding,
                error: None,
                sources: Vec::new(),
                page_refs: Vec::new(),
                created_at,
            });
            s.touch(conversation);
            Ok(id)
        })
    }

    fn finish_message<'a>(
        &'a self,
        id: MessageId,
        answer: nlmx_application::ports::FinishedAnswer<'a>,
    ) -> BoxFuture<'a, Result<(), StorageError>> {
        Box::pin(async move {
            let mut s = self.state.lock().unwrap();
            let Some(m) = s.messages.iter_mut().find(|m| m.id == id) else {
                return Err(StorageError::new("mensagem inexistente"));
            };
            m.content = answer.content.to_string();
            m.status = answer.status;
            m.error = answer.error.map(String::from);
            m.sources = answer.sources.to_vec();
            m.page_refs = answer.page_refs.to_vec();
            let conversation = m.conversation_id;
            s.touch(conversation);
            Ok(())
        })
    }

    fn reset_message(
        &self,
        id: MessageId,
        grounding: AnswerGrounding,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        Box::pin(async move {
            let mut s = self.state.lock().unwrap();
            if let Some(m) = s.messages.iter_mut().find(|m| m.id == id) {
                m.content.clear();
                m.status = MessageStatus::Streaming;
                m.grounding = Some(grounding);
                m.error = None;
                m.sources.clear();
                m.page_refs.clear();
            }
            Ok(())
        })
    }
}

/// The contract every `ConversationRepository` must honour. `document` must exist.
pub async fn conversation_repository_contract(
    repo: &dyn ConversationRepository,
    document: DocumentId,
) {
    let all = repo.create(ConversationScope::Library).await.unwrap();
    assert_eq!(all.scope, ConversationScope::Library);
    let scoped = repo
        .create(ConversationScope::Document(document))
        .await
        .unwrap();
    assert_eq!(scoped.scope, ConversationScope::Document(document));
    assert_ne!(all.id, scoped.id);
    let free = repo.create(ConversationScope::Free).await.unwrap();
    assert_eq!(free.scope, ConversationScope::Free);

    // Activity is ordered by millisecond timestamps: make `all`'s activity strictly later than
    // the creation of the conversations above, or a fast machine ties and the newest id wins.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let q = repo
        .add_message(
            all.id,
            Role::User,
            "Qual a carência?",
            MessageStatus::Answered,
            None,
        )
        .await
        .unwrap();
    let a = repo
        .add_message(
            all.id,
            Role::Assistant,
            "",
            MessageStatus::Streaming,
            Some(AnswerGrounding::Documents),
        )
        .await
        .unwrap();
    repo.set_title(all.id, "Qual a carência?").await.unwrap();
    let boxes = vec![PageBox {
        page: 2,
        bbox: nlmx_domain::document::BoundingBox {
            left: 72.0,
            top: 100.0,
            right: 300.0,
            bottom: 120.0,
        },
    }];
    let reference = |page_start, page_end| {
        let mut r = SourceReference::pdf(
            document,
            "Relatório",
            None,
            page_start,
            page_end,
            boxes.clone(),
        );
        r.section_path = vec!["3. Prazos".into()];
        r
    };
    let source = MessageSource {
        n: 1,
        cited: true,
        document_id: document,
        chunk_id: None,
        document_title: "Relatório".into(),
        page_start: 2,
        page_end: 3,
        section: Some("3. Prazos".into()),
        label: "relatorio.pdf · pp. 2–3".into(),
        quote: "A carência termina após 180 dias.".into(),
        bboxes: boxes.clone(),
        reference: reference(2, 3),
        document_name: "relatorio.pdf".into(),
    };
    let unused = MessageSource {
        n: 2,
        cited: false,
        page_start: 5,
        page_end: 5,
        reference: reference(5, 5),
        ..source.clone()
    };
    let refs = [nlmx_domain::chat::MessagePageRef {
        page: 3,
        document_id: document,
        source: Some(1),
    }];
    repo.finish_message(
        a,
        nlmx_application::ports::FinishedAnswer {
            content: "Termina após 180 dias [1] [página 3].",
            status: MessageStatus::Answered,
            error: None,
            sources: &[source.clone(), unused.clone()],
            page_refs: &refs,
        },
    )
    .await
    .unwrap();

    let messages = repo.messages(all.id).await.unwrap();
    assert_eq!(
        messages.iter().map(|m| m.id).collect::<Vec<_>>(),
        [q, a],
        "oldest first"
    );
    assert_eq!(messages[0].role, Role::User);
    assert_eq!(messages[0].grounding, None);
    assert!(messages[0].sources.is_empty());
    let answer = &messages[1];
    assert_eq!(answer.grounding, Some(AnswerGrounding::Documents));
    assert_eq!(answer.content, "Termina após 180 dias [1] [página 3].");
    assert_eq!(answer.status, MessageStatus::Answered);
    assert_eq!(answer.sources, [source.clone(), unused]);
    assert_eq!(answer.page_refs, refs);

    // The conversation with the latest activity comes first.
    let recent = repo.recent(10).await.unwrap();
    assert_eq!(recent[0].id, all.id);
    assert_eq!(recent[0].title.as_deref(), Some("Qual a carência?"));
    assert_eq!(recent[0].messages, 2);

    // Every outcome round-trips.
    for status in [
        MessageStatus::NotFound,
        MessageStatus::Refused,
        MessageStatus::Cancelled,
        MessageStatus::Failed,
    ] {
        repo.finish_message(
            a,
            nlmx_application::ports::FinishedAnswer {
                content: "x",
                status,
                error: Some("motivo"),
                sources: &[],
                page_refs: &[],
            },
        )
        .await
        .unwrap();
        let m = repo.message(a).await.unwrap().unwrap();
        assert_eq!((m.status, m.error.as_deref()), (status, Some("motivo")));
        assert!(
            m.sources.is_empty() && m.page_refs.is_empty(),
            "sources replaced"
        );
    }

    repo.reset_message(a, AnswerGrounding::Free).await.unwrap();
    let m = repo.message(a).await.unwrap().unwrap();
    assert_eq!(
        (m.status, m.content.as_str(), m.error.clone(), m.grounding),
        (
            MessageStatus::Streaming,
            "",
            None,
            Some(AnswerGrounding::Free)
        )
    );

    for scope in [
        ConversationScope::Document(document),
        ConversationScope::Free,
        ConversationScope::Library,
        ConversationScope::Free,
        ConversationScope::Document(document),
    ] {
        repo.set_scope(all.id, scope).await.unwrap();
        assert_eq!(repo.get(all.id).await.unwrap().unwrap().scope, scope);
    }
    assert_eq!(
        repo.message(a).await.unwrap().unwrap().grounding,
        Some(AnswerGrounding::Free),
        "answers keep their grounding when the scope changes"
    );

    repo.delete(all.id).await.unwrap();
    assert!(repo.get(all.id).await.unwrap().is_none());
    assert!(
        repo.message(a).await.unwrap().is_none(),
        "messages go with the conversation"
    );
    assert!(repo.get(scoped.id).await.unwrap().is_some());
}

/// What every `DocumentRepository` must do when removing documents. `repo` must be empty.
pub async fn document_removal_contract(repo: &dyn DocumentRepository) {
    let new = |sha: char| NewDocument {
        sha256: sha.to_string().repeat(64),
        original_filename: format!("{sha}.pdf"),
        original_path: format!("/tmp/{sha}.pdf"),
        library_path: format!("/library/{sha}.pdf"),
        file_size: 10,
        document_type: nlmx_domain::document_type::DocumentType::Pdf,
        note_text: None,
    };
    let InsertOutcome::Inserted(a) = repo.insert(new('a')).await.unwrap() else {
        panic!("inserted")
    };
    let InsertOutcome::Inserted(b) = repo.insert(new('b')).await.unwrap() else {
        panic!("inserted")
    };

    assert_eq!(repo.remove(9_999).await.unwrap(), None, "unknown document");
    assert_eq!(
        repo.removal_impact(9_999).await.unwrap(),
        RemovalImpact::default()
    );

    let removed = repo.remove(a).await.unwrap().expect("removed");
    assert_eq!(removed.sha256, "a".repeat(64));
    assert_eq!(
        removed.impact,
        RemovalImpact::default(),
        "nothing derived yet"
    );
    assert!(repo.get(a).await.unwrap().is_none());
    assert!(
        repo.find_by_sha256(&"a".repeat(64))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        repo.list()
            .await
            .unwrap()
            .iter()
            .map(|d| d.id)
            .collect::<Vec<_>>(),
        vec![b]
    );
    assert_eq!(repo.hashes().await.unwrap(), vec!["b".repeat(64)]);
    assert!(
        repo.set_status(a, DocumentStatus::Indexed, None)
            .await
            .is_err(),
        "gone"
    );
    assert_eq!(repo.remove(a).await.unwrap(), None, "removing twice");

    // The same file can be imported again, as a new document.
    assert!(matches!(
        repo.insert(new('a')).await.unwrap(),
        InsertOutcome::Inserted(_)
    ));
}

/// What every `IndexingReader` must report about the documents of its `DocumentRepository`.
/// `repo` must be empty.
pub async fn indexing_reader_contract<R>(repo: &R)
where
    R: DocumentRepository + nlmx_application::ports::IndexingReader,
{
    let empty = repo.snapshot(None).await.unwrap();
    assert!(empty.jobs.is_empty());
    assert_eq!((empty.chunks, empty.model.clone()), (0, None));

    let new = |sha: char| NewDocument {
        sha256: sha.to_string().repeat(64),
        original_filename: format!("{sha}.pdf"),
        original_path: format!("/tmp/{sha}.pdf"),
        library_path: format!("/library/{sha}.pdf"),
        file_size: 10,
        document_type: nlmx_domain::document_type::DocumentType::Pdf,
        note_text: None,
    };
    let InsertOutcome::Inserted(a) = repo.insert(new('a')).await.unwrap() else {
        panic!("inserted")
    };
    let InsertOutcome::Inserted(b) = repo.insert(new('b')).await.unwrap() else {
        panic!("inserted")
    };

    // `a` fails twice.
    for _ in 0..2 {
        repo.set_status(a, DocumentStatus::Extracting, None)
            .await
            .unwrap();
        repo.set_status(a, DocumentStatus::Failed, Some("PDF inválido".into()))
            .await
            .unwrap();
    }
    // `b` is read into two chunks and waits for embeddings.
    repo.set_status(b, DocumentStatus::Extracting, None)
        .await
        .unwrap();
    let chunk = |index: u32| ChunkDraft {
        index,
        text: format!("trecho {index}"),
        token_count: 2,
        page_start: 1,
        page_end: 1,
        section_path: Vec::new(),
        boxes: Vec::new(),
        content_hash: format!("{index}").repeat(64),
    };
    repo.save_extraction(
        b,
        Extraction {
            title: "Documento B".into(),
            author: None,
            pdf_created_at: None,
            page_count: 1,
            has_text_layer: true,
            pages: vec![nlmx_application::ports::PageRecord {
                number: 1,
                width: 612.0,
                height: 792.0,
                char_count: 16,
                has_text: true,
            }],
            chunks: vec![chunk(0), chunk(1)],
            extractor_version: 1,
            chunker_version: 1,
            status: DocumentStatus::Embedding,
        },
    )
    .await
    .unwrap();

    let snapshot = repo
        .snapshot(Some("modelo-sem-vetores".into()))
        .await
        .unwrap();
    assert_eq!(
        snapshot
            .jobs
            .iter()
            .map(|j| j.document_id)
            .collect::<Vec<_>>(),
        vec![b, a],
        "most recently imported first"
    );
    let (job_b, job_a) = (&snapshot.jobs[0], &snapshot.jobs[1]);
    assert_eq!(job_a.status, DocumentStatus::Failed);
    assert_eq!(job_a.error.as_deref(), Some("PDF inválido"));
    assert_eq!(job_a.attempts, 2);
    assert_eq!(job_a.chunks, 0);
    assert_eq!(job_b.title, "Documento B");
    assert_eq!(job_b.status, DocumentStatus::Embedding);
    assert_eq!((job_b.chunks, job_b.attempts), (2, 1));
    assert_eq!(snapshot.chunks, 2);
    assert_eq!(snapshot.pending_chunks(), 2);
    assert_eq!(snapshot.model, None, "no vectors for that model");

    repo.remove(a).await.unwrap();
    let snapshot = repo.snapshot(None).await.unwrap();
    assert_eq!(snapshot.jobs.len(), 1, "removed documents are gone");
}

#[cfg(test)]
mod document_contract {
    use super::*;

    #[test]
    fn the_fake_honours_the_removal_contract() {
        llm_contract::block_on(document_removal_contract(&FakeDocumentRepository::default()));
    }

    #[test]
    fn the_fake_honours_the_indexing_reader_contract() {
        llm_contract::block_on(indexing_reader_contract(&FakeDocumentRepository::default()));
    }
}

#[cfg(test)]
mod conversation_contract {
    use super::*;

    #[test]
    fn the_fake_honours_the_contract() {
        llm_contract::block_on(conversation_repository_contract(
            &FakeConversations::default(),
            1,
        ));
    }
}

/// A progress sink that keeps every report, for tests.
#[derive(Default)]
pub struct FakeProgressSink {
    reports: Mutex<Vec<nlmx_domain::ingestion::IngestProgress>>,
}

impl FakeProgressSink {
    pub fn reports(&self) -> Vec<nlmx_domain::ingestion::IngestProgress> {
        self.reports.lock().unwrap().clone()
    }

    /// The phases reported for a document, consecutive repeats collapsed.
    pub fn phases(&self, id: nlmx_domain::ingestion::DocumentId) -> Vec<&'static str> {
        let mut phases: Vec<&'static str> = Vec::new();
        for r in self.reports().into_iter().filter(|r| r.document_id == id) {
            if phases.last() != Some(&r.phase) {
                phases.push(r.phase);
            }
        }
        phases
    }
}

impl nlmx_application::ports::ProgressSink for FakeProgressSink {
    fn report(&self, progress: nlmx_domain::ingestion::IngestProgress) {
        self.reports.lock().unwrap().push(progress);
    }
}
