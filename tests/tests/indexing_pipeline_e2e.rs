//! The indexing pipeline from a file to `indexed`, with every real adapter (parsers, normalizer,
//! chunker, SQLite + FTS5 + vec0, file library) and a deterministic embedder: every format, errors
//! that stay isolated, progress, reindexing without duplicates and files that change.

mod support;

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
    document_type::DocumentType,
    ingestion::{ChunkPolicy, DocumentId, DocumentStatus, ImportOutcome},
    retrieval::{LexicalQuery, RetrievalFilter},
    source::SourceLocation,
};
use nlmx_fs_library::FsLibrary;
use nlmx_normalizer_text::TextNormalizer;
use nlmx_parser_epub::EpubDocumentParser;
use nlmx_parser_text::{CsvDocumentParser, MarkdownDocumentParser, TextDocumentParser};
use nlmx_pdf_pdfium::PdfiumDocumentEngine;
use nlmx_store_sqlite::Database;
use nlmx_structure_heuristic::HeuristicStructureAnalyzer;
use nlmx_testing::{FakeEmbeddingProvider, FakeProgressSink};
use support::{fixture, root, temp_dir};

/// PDFium can be initialized once per process, so every test shares one engine.
fn engine() -> Arc<PdfiumDocumentEngine> {
    static ENGINE: std::sync::OnceLock<Arc<PdfiumDocumentEngine>> = std::sync::OnceLock::new();
    ENGINE
        .get_or_init(|| {
            Arc::new(PdfiumDocumentEngine::from_default_location().expect("make bootstrap"))
        })
        .clone()
}

/// The embedding model of the app, which a test can install or take away.
#[derive(Default)]
struct SwitchableModel(Mutex<Option<Arc<dyn EmbeddingProvider>>>);

impl SwitchableModel {
    fn install(&self) {
        *self.0.lock().unwrap() = Some(Arc::new(FakeEmbeddingProvider { dimensions: 64 }));
    }
}

impl EmbeddingSource for SwitchableModel {
    fn current(&self) -> Option<Arc<dyn EmbeddingProvider>> {
        self.0.lock().unwrap().clone()
    }
}

struct App {
    dir: PathBuf,
    db: Arc<Database>,
    ingestion: DocumentIngestion,
    embedder: Arc<EmbedDocuments>,
    model: Arc<SwitchableModel>,
    progress: Arc<FakeProgressSink>,
}

impl App {
    fn new(name: &str, with_model: bool) -> Self {
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
    fn user_file(&self, source: &Path, name: &str) -> PathBuf {
        let inbox = self.dir.join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let path = inbox.join(name);
        std::fs::copy(source, &path).unwrap();
        path
    }

    fn sql(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.dir.join("nlmx.sqlite3")).unwrap()
    }

    fn count(&self, query: &str) -> i64 {
        self.sql().query_row(query, [], |r| r.get(0)).unwrap()
    }

    /// `[chunks, embeddings, lexical entries]` of the whole database.
    fn index_sizes(&self) -> [i64; 3] {
        [
            self.count("SELECT count(*) FROM document_chunks"),
            self.count("SELECT count(*) FROM chunk_embeddings"),
            self.count("SELECT count(*) FROM document_chunks_fts_docsize"),
        ]
    }

    fn library_files(&self) -> Vec<String> {
        std::fs::read_dir(self.dir.join("library"))
            .map(|entries| {
                entries
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn status(&self, id: DocumentId) -> DocumentStatus {
        self.db.get(id).await.unwrap().unwrap().status
    }

    async fn hits(&self, word: &str) -> Vec<DocumentId> {
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

fn text_fixture(name: &str) -> PathBuf {
    root()
        .join("crates/adapters/parser-text/tests/fixtures")
        .join(name)
}

fn epub_fixture(name: &str) -> PathBuf {
    root()
        .join("crates/adapters/parser-epub/tests/fixtures")
        .join(name)
}

fn imported(outcome: ImportOutcome) -> (DocumentId, u32, DocumentStatus) {
    match outcome {
        ImportOutcome::Imported { id, chunks, status } => (id, chunks, status),
        other => panic!("{other:?}"),
    }
}

/// The first word of at least six letters of a text: something the lexical index must find.
fn a_word_of(text: &str) -> String {
    text.split(|c: char| !c.is_alphabetic())
        .find(|w| w.chars().count() >= 6)
        .unwrap_or_else(|| panic!("no long word in {text:?}"))
        .to_string()
}

#[tokio::test]
async fn every_format_goes_from_file_to_indexed() {
    let app = App::new("formats", true);
    let cases: [(DocumentType, PathBuf, &str); 5] = [
        (DocumentType::Pdf, fixture("report.pdf"), "report.pdf"),
        (DocumentType::Markdown, text_fixture("guia.md"), "guia.md"),
        (DocumentType::Text, text_fixture("notas.txt"), "notas.txt"),
        (DocumentType::Csv, text_fixture("vendas.csv"), "vendas.csv"),
        (DocumentType::Epub, epub_fixture("livro.epub"), "livro.epub"),
    ];
    for (kind, source, name) in cases {
        let path = app.user_file(&source, name);
        let (id, chunks, status) = imported(app.ingestion.import(&path).await);
        assert_eq!(status, DocumentStatus::Indexed, "{name}");
        assert!(chunks > 0, "{name}");

        let record = app.db.get(id).await.unwrap().unwrap();
        assert_eq!(record.document_type, kind, "{name}");
        assert_eq!(record.status, DocumentStatus::Indexed, "{name}");
        assert!(
            Path::new(&record.library_path).exists(),
            "{name}: the library keeps a copy"
        );

        let stored = app.db.chunks_of(id).await.unwrap();
        assert_eq!(stored.len() as u32, chunks, "{name}");
        for chunk in &stored {
            let matches_format = matches!(
                (&chunk.location, kind),
                (SourceLocation::Pdf { .. }, DocumentType::Pdf)
                    | (SourceLocation::Markdown { .. }, DocumentType::Markdown)
                    | (SourceLocation::Text { .. }, DocumentType::Text)
                    | (SourceLocation::Csv { .. }, DocumentType::Csv)
                    | (SourceLocation::Epub { .. }, DocumentType::Epub)
            );
            assert!(matches_format, "{name}: {:?}", chunk.location);
        }
        // Every chunk is embedded and findable by its words.
        let embedded: i64 = app
            .sql()
            .query_row(
                "SELECT count(*) FROM chunk_embeddings e JOIN document_chunks c ON c.id = e.chunk_id WHERE c.document_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(embedded as u32, chunks, "{name}");
        let word = a_word_of(&stored[0].text);
        assert!(app.hits(&word).await.contains(&id), "{name}: {word}");
    }
    assert_eq!(app.library_files().len(), 5);
}

#[tokio::test]
async fn one_failure_does_not_stop_the_others() {
    let app = App::new("isolation", true);
    let files = [
        app.user_file(&fixture("corrupt.pdf"), "quebrado.pdf"),
        app.user_file(&text_fixture("guia.md"), "guia.md"),
        app.user_file(&epub_fixture("drm.epub"), "protegido.epub"),
        app.user_file(&text_fixture("vazio.txt"), "vazio.txt"),
        app.user_file(&text_fixture("notas.txt"), "notas.txt"),
        app.user_file(&text_fixture("vendas.csv"), "dados.xyz"), // no parser for it
        app.user_file(&epub_fixture("livro.epub"), "livro.epub"),
    ];
    let outcomes = app.ingestion.import_many(&files).await;
    assert_eq!(outcomes.len(), files.len());

    let by_name = |name: &str| {
        &outcomes
            .iter()
            .find(|(path, _)| path.file_name().unwrap().to_str() == Some(name))
            .unwrap()
            .1
    };
    for good in ["guia.md", "notas.txt", "livro.epub"] {
        assert!(
            matches!(
                by_name(good),
                ImportOutcome::Imported {
                    status: DocumentStatus::Indexed,
                    ..
                }
            ),
            "{good}: {:?}",
            by_name(good)
        );
    }
    for bad in ["quebrado.pdf", "protegido.epub", "vazio.txt"] {
        let ImportOutcome::Failed {
            id: Some(id),
            reason,
        } = by_name(bad)
        else {
            panic!("{bad}: {:?}", by_name(bad))
        };
        assert!(!reason.is_empty(), "{bad}");
        let record = app.db.get(*id).await.unwrap().unwrap();
        assert_eq!(record.status, DocumentStatus::Failed, "{bad}");
        // The diagnosis is kept with the document.
        let error: Option<String> = app
            .sql()
            .query_row("SELECT error FROM documents WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(error.is_some_and(|e| !e.is_empty()), "{bad}");
    }
    // An unknown format is refused before anything is copied or recorded.
    assert!(matches!(
        by_name("dados.xyz"),
        ImportOutcome::Failed { id: None, .. }
    ));
    assert_eq!(app.count("SELECT count(*) FROM documents"), 6);
    assert_eq!(app.library_files().len(), 6);
}

#[tokio::test]
async fn status_and_progress_follow_each_stage() {
    let app = App::new("progress", true);
    let path = app.user_file(&text_fixture("guia.md"), "guia.md");
    let (id, chunks, _) = imported(app.ingestion.import(&path).await);

    assert_eq!(
        app.progress.phases(id),
        [
            "parsing",
            "structuring",
            "chunking",
            "saving",
            "embedding",
            "indexed"
        ]
    );
    let reports: Vec<_> = app
        .progress
        .reports()
        .into_iter()
        .filter(|r| r.document_id == id)
        .collect();
    assert!(reports.iter().all(|r| r.file_name == "guia.md"));
    assert!(reports.iter().all(|r| r.format == DocumentType::Markdown));
    assert!(
        reports.windows(2).all(|w| w[0].fraction <= w[1].fraction),
        "progress never goes back: {:?}",
        reports.iter().map(|r| r.fraction).collect::<Vec<_>>()
    );
    assert_eq!(reports.last().unwrap().fraction, 1.0);
    assert_eq!(reports.last().unwrap().chunks, Some(chunks));
    assert_eq!(reports.last().unwrap().status, "indexed");
    // Chunks are only known once the document is chunked.
    assert_eq!(reports.first().unwrap().chunks, None);

    // A failure is reported too, and the document says why.
    let bad = app.user_file(&text_fixture("vazio.txt"), "vazio.txt");
    let ImportOutcome::Failed {
        id: Some(bad_id), ..
    } = app.ingestion.import(&bad).await
    else {
        panic!("expected a failure")
    };
    assert_eq!(app.progress.phases(bad_id).last(), Some(&"failed"));
    assert_eq!(app.status(bad_id).await, DocumentStatus::Failed);
}

#[tokio::test]
async fn without_a_model_documents_wait_and_are_indexed_later() {
    let app = App::new("no-model", false);
    let path = app.user_file(&epub_fixture("livro.epub"), "livro.epub");
    let (id, chunks, status) = imported(app.ingestion.import(&path).await);
    assert_eq!(status, DocumentStatus::Embedding);
    assert_eq!(app.progress.phases(id).last(), Some(&"waiting"));
    assert_eq!(app.count("SELECT count(*) FROM chunk_embeddings"), 0);
    // Search is lexical-only meanwhile.
    let stored = app.db.chunks_of(id).await.unwrap();
    assert!(app.hits(&a_word_of(&stored[0].text)).await.contains(&id));

    app.model.install();
    let outcomes = app.embedder.embed_pending().await;
    assert_eq!(outcomes.len(), 1);
    assert_eq!(app.status(id).await, DocumentStatus::Indexed);
    assert_eq!(
        app.count("SELECT count(*) FROM chunk_embeddings") as u32,
        chunks
    );
}

#[tokio::test]
async fn reindexing_never_duplicates_chunks_or_embeddings() {
    let app = App::new("reindex", true);
    let mut ids = Vec::new();
    for (source, name) in [
        (fixture("report.pdf"), "report.pdf"),
        (text_fixture("guia.md"), "guia.md"),
        (text_fixture("vendas.csv"), "vendas.csv"),
        (epub_fixture("livro.epub"), "livro.epub"),
    ] {
        let path = app.user_file(&source, name);
        ids.push(imported(app.ingestion.import(&path).await).0);
    }
    let before = app.index_sizes();
    assert!(before.iter().all(|n| *n > 0), "{before:?}");

    for _ in 0..2 {
        for id in &ids {
            let (same, _, status) = imported(app.ingestion.reindex(*id).await);
            assert_eq!(same, *id);
            assert_eq!(status, DocumentStatus::Indexed);
        }
        assert_eq!(app.index_sizes(), before);
        assert_eq!(
            app.count("SELECT count(*) FROM documents") as usize,
            ids.len()
        );
    }
    // Importing the same file again is a duplicate, not a second copy.
    let again = app.user_file(&text_fixture("guia.md"), "guia.md");
    assert!(matches!(
        app.ingestion.import(&again).await,
        ImportOutcome::Duplicate { .. }
    ));
    assert_eq!(app.index_sizes(), before);
}

#[tokio::test]
async fn a_changed_file_updates_the_same_document() {
    let app = App::new("changed", true);
    let path = app.dir.join("nota.md");
    std::fs::write(
        &path,
        "# Nota\n\nO texto original fala sobre abacaxi e mais nada.\n",
    )
    .unwrap();
    let (id, _, _) = imported(app.ingestion.import(&path).await);
    assert!(app.hits("abacaxi").await.contains(&id));
    let first_copy = app.library_files();
    assert_eq!(first_copy.len(), 1);

    std::fs::write(
        &path,
        "# Nota\n\nO texto novo fala sobre melancia e mais nada.\n",
    )
    .unwrap();
    let (same, _, status) = imported(app.ingestion.import(&path).await);
    assert_eq!(same, id, "the document keeps its id");
    assert_eq!(status, DocumentStatus::Indexed);

    assert!(app.hits("melancia").await.contains(&id));
    assert!(app.hits("abacaxi").await.is_empty(), "the old text is gone");
    assert_eq!(app.count("SELECT count(*) FROM documents"), 1);
    let chunks = app.count("SELECT count(*) FROM document_chunks");
    assert_eq!(app.count("SELECT count(*) FROM chunk_embeddings"), chunks);
    // Only the new version is kept in the library.
    let library = app.library_files();
    assert_eq!(library.len(), 1);
    assert_ne!(library, first_copy);

    // The same content at another path is a duplicate of the document, not a new one.
    let copy = app.dir.join("copia.md");
    std::fs::copy(&path, &copy).unwrap();
    assert!(matches!(
        app.ingestion.import(&copy).await,
        ImportOutcome::Duplicate { .. }
    ));
    assert_eq!(app.count("SELECT count(*) FROM documents"), 1);
}

#[tokio::test]
async fn the_library_keeps_each_format_with_its_extension() {
    let app = App::new("extensions", true);
    for (source, name) in [
        (text_fixture("guia.md"), "guia.md"),
        (epub_fixture("livro.epub"), "livro.epub"),
        (fixture("report.pdf"), "report.pdf"),
    ] {
        let path = app.user_file(&source, name);
        imported(app.ingestion.import(&path).await);
    }
    let mut extensions: Vec<String> = app
        .library_files()
        .iter()
        .map(|f| f.rsplit('.').next().unwrap().to_string())
        .collect();
    extensions.sort();
    assert_eq!(extensions, ["epub", "md", "pdf"]);
}

/// A marker in the text, the title and the file name of every format (and in files that fail)
/// never reaches the logs (captured at TRACE through the app's redacting layer) or the metrics.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_from_any_format_reaches_logs() {
    use nlmx_telemetry::{Format, LogLayer, MemorySink, MetricsLayer, MetricsRegistry};
    use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt};

    const CANARY: &str = "CANARIO-9c1e";
    let sink = MemorySink::default();
    let metrics = MetricsRegistry::new();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(LogLayer::new(sink.clone(), Format::Json).with_filter(EnvFilter::new("trace")))
            .with(MetricsLayer::new(metrics.clone())),
    )
    .unwrap();

    let app = App::new("canary", true);
    let inbox = app.dir.join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    let write = |name: &str, content: &str| {
        let path = inbox.join(format!("{CANARY} {name}"));
        std::fs::write(&path, content).unwrap();
        path
    };
    let files = [
        write(
            "nota.md",
            &format!("# Título {CANARY}\n\nO paciente {CANARY} tem carência.\n"),
        ),
        write(
            "nota.txt",
            &format!("Prontuário {CANARY}\n\nTexto sobre {CANARY} e mais palavras.\n"),
        ),
        write("dados.csv", &format!("nome;valor\n{CANARY};10\noutro;20\n")),
        write("vazio.txt", ""),
        write("quebrado.pdf", &format!("%PDF-1.7\n{CANARY} garbage")),
        inbox.join(format!("{CANARY} ausente.md")),
        write("sem-formato.xyz", CANARY),
    ];
    let outcomes = app.ingestion.import_many(&files).await;
    assert!(
        outcomes
            .iter()
            .any(|(_, o)| matches!(o, ImportOutcome::Imported { .. }))
    );
    // Reindexing and a changed file go through the same paths.
    let (md, ..) = imported(app.ingestion.reindex(1).await);
    let _ = md;
    std::fs::write(
        &files[0],
        format!("# Novo {CANARY}\n\nOutro texto de {CANARY}.\n"),
    )
    .unwrap();
    let _ = app.ingestion.import(&files[0]).await;

    let lines = sink.0.lock().unwrap();
    assert!(!lines.is_empty(), "logs were captured");
    let leaks: Vec<&String> = lines.iter().filter(|l| l.contains(CANARY)).collect();
    assert!(leaks.is_empty(), "canary in logs: {leaks:#?}");
    let metrics_json = nlmx_telemetry::snapshot_json(&metrics.snapshot()).to_string();
    assert!(!metrics_json.contains(CANARY));
}
