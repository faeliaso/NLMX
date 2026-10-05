//! Shared by the workspace tests: a library of fixture PDFs ingested with the real adapters
//! (PDFium, structure analysis, chunker, SQLite) in a temporary directory.

#![allow(dead_code)]

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use nlmx_application::{
    ports::{EmbeddingSource, LlmProvider},
    services::{
        free_chat::FreeChat,
        parsing::{ParserRegistry, PdfDocumentParser},
        pipeline::ContentPipeline,
        rag::{RagEngine, RagOptions},
        retrieval::HybridRetriever,
        retriever::Retriever,
    },
    use_cases::{ChatService, DocumentIngestion, EmbedDocuments, ViewDocument},
};
use nlmx_chunker_structural::{HeuristicTokenCounter, MultiFormatChunker, StructuralChunker};
use nlmx_domain::ingestion::{ChunkPolicy, ImportOutcome};
use nlmx_fs_library::FsLibrary;
use nlmx_normalizer_text::TextNormalizer;
use nlmx_parser_epub::EpubDocumentParser;
use nlmx_parser_office::{DocxDocumentParser, XlsxDocumentParser};
use nlmx_parser_text::{CsvDocumentParser, MarkdownDocumentParser, TextDocumentParser};
use nlmx_pdf_pdfium::PdfiumDocumentEngine;
use nlmx_store_sqlite::Database;
use nlmx_structure_heuristic::HeuristicStructureAnalyzer;
use nlmx_testing::{FakeEmbeddingProvider, FixedEmbeddingSource};

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

pub fn fixture(name: &str) -> PathBuf {
    root()
        .join("crates/adapters/pdf-pdfium/tests/fixtures")
        .join(name)
}

pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nlmx-ws-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Hash-based embedder: deterministic, not semantic (search is then mostly lexical).
pub fn deterministic_embeddings() -> Arc<FixedEmbeddingSource> {
    FixedEmbeddingSource::of(FakeEmbeddingProvider { dimensions: 64 })
}

/// PDFium can be initialized once per process: every test of a binary shares one engine.
pub fn shared_engine() -> Arc<PdfiumDocumentEngine> {
    static ENGINE: std::sync::OnceLock<Arc<PdfiumDocumentEngine>> = std::sync::OnceLock::new();
    ENGINE
        .get_or_init(|| {
            Arc::new(PdfiumDocumentEngine::from_default_location().expect("make bootstrap"))
        })
        .clone()
}

/// The content pipeline exactly as the app composes it (`src-tauri/src/wiring.rs`): every
/// parser, the text normalizer and the multi-format chunker. A PDF goes through this in
/// production, so the tests of PDF behaviour must too.
pub fn production_pipeline(engine: Arc<PdfiumDocumentEngine>) -> Arc<ContentPipeline> {
    let parsers = ParserRegistry::new()
        .with(Arc::new(PdfDocumentParser::new(
            engine,
            Arc::new(HeuristicStructureAnalyzer),
        )))
        .with(Arc::new(MarkdownDocumentParser))
        .with(Arc::new(TextDocumentParser))
        .with(Arc::new(CsvDocumentParser))
        .with(Arc::new(EpubDocumentParser::default()))
        .with(Arc::new(DocxDocumentParser::default()))
        .with(Arc::new(XlsxDocumentParser::default()));
    Arc::new(ContentPipeline {
        parsers,
        normalizer: Arc::new(TextNormalizer),
        chunker: Arc::new(MultiFormatChunker),
        tokens: Arc::new(HeuristicTokenCounter),
        policy: ChunkPolicy::default(),
    })
}

pub struct Library {
    pub dir: PathBuf,
    pub db: Arc<Database>,
    pub engine: Arc<PdfiumDocumentEngine>,
    pub ingestion: DocumentIngestion,
    pub embeddings: Arc<dyn EmbeddingSource>,
    /// Imported document ids by fixture file name.
    pub documents: HashMap<String, i64>,
}

impl Library {
    /// A library ingested the way the app does (the content pipeline).
    pub fn new(dir: PathBuf, embeddings: Arc<dyn EmbeddingSource>) -> Self {
        Self::build(dir, embeddings, true)
    }

    /// A library ingested by the legacy PDF-only path (only to compare it with the real one).
    pub fn legacy(dir: PathBuf, embeddings: Arc<dyn EmbeddingSource>) -> Self {
        Self::build(dir, embeddings, false)
    }

    fn build(dir: PathBuf, embeddings: Arc<dyn EmbeddingSource>, production: bool) -> Self {
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Database::open(dir.join("nlmx.sqlite3")).unwrap());
        let engine = shared_engine();
        let ingestion = DocumentIngestion {
            pipeline: production.then(|| production_pipeline(engine.clone())),
            progress: None,
            viewer: None,
            engine: engine.clone(),
            files: Arc::new(FsLibrary::new(dir.join("library"))),
            documents: db.clone(),
            analyzer: Arc::new(HeuristicStructureAnalyzer),
            chunker: Arc::new(StructuralChunker),
            tokens: Arc::new(HeuristicTokenCounter),
            policy: ChunkPolicy::default(),
            embedder: Some(Arc::new(EmbedDocuments {
                progress: None,
                embeddings: embeddings.clone(),
                vectors: db.clone(),
                chunks: db.clone(),
                documents: db.clone(),
                batch_size: 32,
            })),
        };
        Self {
            dir,
            db,
            engine,
            ingestion,
            embeddings,
            documents: HashMap::new(),
        }
    }

    /// Imports fixture files (panics on failure: the corpus must be valid).
    pub async fn import(&mut self, files: &[&str]) {
        for file in files {
            match self.ingestion.import(&fixture(file)).await {
                ImportOutcome::Imported { id, .. } | ImportOutcome::Duplicate { id } => {
                    self.documents.insert(file.to_string(), id);
                }
                other => panic!("{file}: {other:?}"),
            }
        }
    }

    pub fn retriever(&self) -> Arc<Retriever> {
        Arc::new(Retriever::new(Arc::new(HybridRetriever::new(
            self.embeddings.clone(),
            self.db.clone(),
            self.db.clone(),
            self.db.clone(),
        ))))
    }

    pub fn rag(&self, llm: Arc<dyn LlmProvider>) -> RagEngine {
        RagEngine::new(self.retriever(), self.db.clone(), llm)
    }

    pub fn chat(&self, llm: Arc<dyn LlmProvider>, options: RagOptions) -> ChatService {
        ChatService {
            conversations: self.db.clone(),
            rag: Arc::new(self.rag(llm.clone())),
            free: Arc::new(FreeChat::new(llm)),
            options,
        }
    }

    pub fn viewer(&self) -> ViewDocument {
        ViewDocument::new(self.engine.clone(), self.db.clone())
    }

    /// The fixture file of a document id.
    pub fn file_of(&self, id: i64) -> &str {
        self.documents
            .iter()
            .find(|(_, v)| **v == id)
            .map(|(k, _)| k.as_str())
            .unwrap_or("?")
    }
}
