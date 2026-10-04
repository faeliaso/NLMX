//! A library built by the real indexing pipeline (every parser, normalizer, chunker, SQLite with
//! FTS5 and vec0, the file library) with a deterministic embedder and a progress sink.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use nlmx_application::{
    ports::{DocumentRepository, EmbeddingProvider, EmbeddingSource, LexicalIndex, ProgressSink},
    services::{
        parsing::{ParserRegistry, PdfDocumentParser},
        pipeline::ContentPipeline,
    },
    use_cases::{DocumentIngestion, EmbedDocuments},
};
use nlmx_chunker_structural::{HeuristicTokenCounter, MultiFormatChunker};
use nlmx_domain::{
    ingestion::{ChunkPolicy, DocumentId, DocumentStatus, ImportOutcome},
    retrieval::{LexicalQuery, RetrievalFilter},
};
use nlmx_fs_library::FsLibrary;
use nlmx_normalizer_text::TextNormalizer;
use nlmx_parser_epub::EpubDocumentParser;
use nlmx_parser_text::{CsvDocumentParser, MarkdownDocumentParser, TextDocumentParser};
use nlmx_pdf_pdfium::PdfiumDocumentEngine;
use nlmx_store_sqlite::Database;
use nlmx_structure_heuristic::HeuristicStructureAnalyzer;
use nlmx_testing::{FakeEmbeddingProvider, FakeProgressSink};

use super::{root, temp_dir};

/// PDFium can be initialized once per process, so every test shares one engine.
pub fn engine() -> Arc<PdfiumDocumentEngine> {
    static ENGINE: std::sync::OnceLock<Arc<PdfiumDocumentEngine>> = std::sync::OnceLock::new();
    ENGINE
        .get_or_init(|| {
            Arc::new(PdfiumDocumentEngine::from_default_location().expect("make bootstrap"))
        })
        .clone()
}

/// The embedding model of the app, which a test can install or take away.
#[derive(Default)]
pub struct SwitchableModel(Mutex<Option<Arc<dyn EmbeddingProvider>>>);

impl SwitchableModel {
    pub fn install(&self) {
        *self.0.lock().unwrap() = Some(Arc::new(FakeEmbeddingProvider { dimensions: 64 }));
    }
}

impl EmbeddingSource for SwitchableModel {
    fn current(&self) -> Option<Arc<dyn EmbeddingProvider>> {
        self.0.lock().unwrap().clone()
    }
}

pub struct App {
    pub dir: PathBuf,
    pub db: Arc<Database>,
    pub ingestion: DocumentIngestion,
    pub embedder: Arc<EmbedDocuments>,
    pub model: Arc<SwitchableModel>,
    pub progress: Arc<FakeProgressSink>,
}

impl App {
    pub fn new(name: &str, with_model: bool) -> Self {
        let dir = temp_dir(name);
        let db = Arc::new(Database::open(dir.join("nlmx.sqlite3")).unwrap());
        let model = Arc::new(SwitchableModel::default());
        if with_model {
            model.install();
        }
        let progress = Arc::new(FakeProgressSink::default());
        let sink: Arc<dyn ProgressSink> = progress.clone();
        let embedder = Arc::new(EmbedDocuments {
            progress: Some(sink.clone()),
            embeddings: model.clone(),
            vectors: db.clone(),
            chunks: db.clone(),
            documents: db.clone(),
            batch_size: 4,
        });
        let engine = engine();
        let parsers = ParserRegistry::new()
            .with(Arc::new(PdfDocumentParser::new(
                engine.clone(),
                Arc::new(HeuristicStructureAnalyzer),
            )))
            .with(Arc::new(MarkdownDocumentParser))
            .with(Arc::new(TextDocumentParser))
            .with(Arc::new(CsvDocumentParser))
            .with(Arc::new(EpubDocumentParser::default()));
        let tokens = Arc::new(HeuristicTokenCounter);
        let ingestion = DocumentIngestion {
            pipeline: Some(Arc::new(ContentPipeline {
                parsers,
                normalizer: Arc::new(TextNormalizer),
                chunker: Arc::new(MultiFormatChunker),
                tokens: tokens.clone(),
                policy: ChunkPolicy::default(),
            })),
            progress: Some(sink),
            viewer: None,
            engine,
            files: Arc::new(FsLibrary::new(dir.join("library"))),
            documents: db.clone(),
            analyzer: Arc::new(HeuristicStructureAnalyzer),
            chunker: Arc::new(nlmx_chunker_structural::StructuralChunker),
            tokens,
            policy: ChunkPolicy::default(),
            embedder: Some(embedder.clone()),
        };
        Self {
            dir,
            db,
            ingestion,
            embedder,
            model,
            progress,
        }
    }

    /// A file of the user's, copied to a folder of its own (some tests edit it).
    pub fn user_file(&self, source: &Path, name: &str) -> PathBuf {
        let inbox = self.dir.join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let path = inbox.join(name);
        std::fs::copy(source, &path).unwrap();
        path
    }

    pub fn sql(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.dir.join("nlmx.sqlite3")).unwrap()
    }

    pub fn count(&self, query: &str) -> i64 {
        self.sql().query_row(query, [], |r| r.get(0)).unwrap()
    }

    /// `[chunks, embeddings, lexical entries]` of the whole database.
    pub fn index_sizes(&self) -> [i64; 3] {
        [
            self.count("SELECT count(*) FROM document_chunks"),
            self.count("SELECT count(*) FROM chunk_embeddings"),
            self.count("SELECT count(*) FROM document_chunks_fts_docsize"),
        ]
    }

    pub fn library_files(&self) -> Vec<String> {
        std::fs::read_dir(self.dir.join("library"))
            .map(|entries| {
                entries
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub async fn status(&self, id: DocumentId) -> DocumentStatus {
        self.db.get(id).await.unwrap().unwrap().status
    }

    pub async fn hits(&self, word: &str) -> Vec<DocumentId> {
        LexicalIndex::search(
            self.db.as_ref(),
            &LexicalQuery::from_query(word),
            20,
            &RetrievalFilter::default(),
        )
        .await
        .unwrap()
        .into_iter()
        .map(|h| h.document_id)
        .collect()
    }
}

pub fn text_fixture(name: &str) -> PathBuf {
    root()
        .join("crates/adapters/parser-text/tests/fixtures")
        .join(name)
}

pub fn epub_fixture(name: &str) -> PathBuf {
    root()
        .join("crates/adapters/parser-epub/tests/fixtures")
        .join(name)
}

pub fn imported(outcome: ImportOutcome) -> (DocumentId, u32, DocumentStatus) {
    match outcome {
        ImportOutcome::Imported { id, chunks, status } => (id, chunks, status),
        other => panic!("{other:?}"),
    }
}

/// The first word of at least six letters of a text: something the lexical index must find.
pub fn a_word_of(text: &str) -> String {
    text.split(|c: char| !c.is_alphabetic())
        .find(|w| w.chars().count() >= 6)
        .unwrap_or_else(|| panic!("no long word in {text:?}"))
        .to_string()
}
