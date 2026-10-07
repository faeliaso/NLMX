//! `DocumentIngestion` with in-memory fakes: flow, statuses, duplicates and idempotency.

use std::{path::Path, sync::Arc};

use nlmx_application::use_cases::{DocumentIngestion, Enqueued};
use nlmx_domain::{
    document::{BoundingBox, DocumentMetadata, PageImage, TextSpan},
    ingestion::{ChunkPolicy, DocumentStatus, ImportOutcome},
};
use nlmx_testing::{
    FakeChunker, FakeDocumentEngine, FakeDocumentRepository, FakeFileStore, FakePage,
    FakeStructureAnalyzer, WordTokenCounter,
};

fn span(text: &str) -> TextSpan {
    TextSpan {
        text: text.into(),
        bbox: BoundingBox {
            left: 72.0,
            top: 60.0,
            right: 300.0,
            bottom: 75.0,
        },
        font_name: "Helvetica".into(),
        font_size: 11.0,
        bold: false,
        italic: false,
    }
}

fn text_page(lines: &[&str]) -> FakePage {
    FakePage {
        spans: lines.iter().map(|l| span(l)).collect(),
        images: vec![],
    }
}

fn image_page() -> FakePage {
    FakePage {
        spans: vec![],
        images: vec![PageImage {
            index: 0,
            bbox: BoundingBox {
                left: 0.0,
                top: 0.0,
                right: 612.0,
                bottom: 792.0,
            },
            width_px: 10,
            height_px: 10,
            png: vec![],
        }],
    }
}

struct Setup {
    ingestion: DocumentIngestion,
    files: Arc<FakeFileStore>,
    documents: Arc<FakeDocumentRepository>,
}

/// (source path, file bytes, PDF title, pages)
type FakeFile<'a> = (&'a str, &'a [u8], Option<&'a str>, Vec<FakePage>);

/// Registers `content` at `path`, and the engine document at the library path its hash maps to.
fn setup(files_and_pages: Vec<FakeFile<'_>>) -> Setup {
    let files = Arc::new(FakeFileStore::default());
    let mut engine = FakeDocumentEngine::default();
    for (path, content, title, pages) in files_and_pages {
        let sha = files.add(path, content);
        let metadata = DocumentMetadata {
            title: title.map(String::from),
            author: Some("Autora".into()),
            ..Default::default()
        };
        engine = engine.with_document(FakeFileStore::library_path(&sha), metadata, pages);
    }
    let documents = Arc::new(FakeDocumentRepository::default());
    let ingestion = DocumentIngestion {
        pipeline: None,
        progress: None,
        viewer: None,
        engine: Arc::new(engine),
        files: files.clone(),
        documents: documents.clone(),
        analyzer: Arc::new(FakeStructureAnalyzer),
        chunker: Arc::new(FakeChunker),
        tokens: Arc::new(WordTokenCounter),
        policy: ChunkPolicy::default(),
        embedder: None,
    };
    Setup {
        ingestion,
        files,
        documents,
    }
}

#[tokio::test]
async fn imports_a_pdf_into_chunks_waiting_for_embeddings() {
    let s = setup(vec![(
        "/in/contrato.pdf",
        b"pdf-1",
        Some("Contrato"),
        vec![
            text_page(&["Carência de 180 dias.", "Cobertura total."]),
            text_page(&["Página dois."]),
        ],
    )]);

    let outcome = s.ingestion.import(Path::new("/in/contrato.pdf")).await;
    assert_eq!(
        outcome,
        ImportOutcome::Imported {
            id: 1,
            chunks: 3,
            status: DocumentStatus::Embedding
        }
    );

    let row = s.documents.row(1);
    assert_eq!(
        row.history,
        [
            DocumentStatus::Queued,
            DocumentStatus::Extracting,
            DocumentStatus::Structuring,
            DocumentStatus::Chunking,
            DocumentStatus::Embedding
        ]
    );
    assert_eq!(row.new.original_filename, "contrato.pdf");
    assert_eq!(row.new.original_path, "/in/contrato.pdf");
    let extraction = row.extraction.unwrap();
    assert_eq!(
        (extraction.title.as_str(), extraction.author.as_deref()),
        ("Contrato", Some("Autora"))
    );
    assert_eq!(extraction.page_count, 2);
    assert_eq!(extraction.pages.len(), 2);
    assert!(extraction.has_text_layer);
    assert_eq!(
        (extraction.extractor_version, extraction.chunker_version),
        (99, 98)
    );
    let pages: Vec<u32> = extraction.chunks.iter().map(|c| c.page_start).collect();
    assert_eq!(pages, [1, 1, 2]);
    assert_eq!(extraction.chunks[0].boxes[0].page, 1);
}

#[tokio::test]
async fn the_same_file_imported_again_is_a_duplicate_and_changes_nothing() {
    let s = setup(vec![
        (
            "/in/a.pdf",
            b"same bytes",
            Some("A"),
            vec![text_page(&["Texto."])],
        ),
        (
            "/elsewhere/copy-of-a.pdf",
            b"same bytes",
            Some("A"),
            vec![text_page(&["Texto."])],
        ),
    ]);
    assert!(matches!(
        s.ingestion.import(Path::new("/in/a.pdf")).await,
        ImportOutcome::Imported { id: 1, .. }
    ));
    let before = s.documents.row(1);

    for path in ["/in/a.pdf", "/elsewhere/copy-of-a.pdf"] {
        assert_eq!(
            s.ingestion.import(Path::new(path)).await,
            ImportOutcome::Duplicate { id: 1 }
        );
    }
    assert_eq!(s.documents.len(), 1);
    assert_eq!(s.files.store_calls(), 1, "the library copy is made once");
    assert_eq!(
        s.documents.row(1).history,
        before.history,
        "no re-processing"
    );
}

#[tokio::test]
async fn re_running_ingestion_produces_identical_chunks() {
    let s = setup(vec![(
        "/in/a.pdf",
        b"x",
        None,
        vec![text_page(&["Um.", "Dois."]), text_page(&["Três."])],
    )]);
    s.ingestion.import(Path::new("/in/a.pdf")).await;
    let first = s.documents.row(1).extraction.unwrap();

    assert!(matches!(
        s.ingestion.ingest(1).await,
        ImportOutcome::Imported { chunks: 3, .. }
    ));
    let second = s.documents.row(1).extraction.unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn title_falls_back_to_the_file_name() {
    let s = setup(vec![(
        "/in/relatorio_anual-2026.pdf",
        b"x",
        None,
        vec![text_page(&["Corpo."])],
    )]);
    s.ingestion
        .import(Path::new("/in/relatorio_anual-2026.pdf"))
        .await;
    assert_eq!(
        s.documents.row(1).extraction.unwrap().title,
        "relatorio anual 2026"
    );
}

#[tokio::test]
async fn image_only_documents_need_ocr() {
    let s = setup(vec![(
        "/in/scan.pdf",
        b"scan",
        None,
        vec![image_page(), image_page()],
    )]);
    let outcome = s.ingestion.import(Path::new("/in/scan.pdf")).await;
    assert_eq!(
        outcome,
        ImportOutcome::Imported {
            id: 1,
            chunks: 0,
            status: DocumentStatus::NeedsOcr
        }
    );
    let extraction = s.documents.row(1).extraction.unwrap();
    assert!(!extraction.has_text_layer);
    assert!(extraction.pages.iter().all(|p| !p.has_text));
}

#[tokio::test]
async fn engine_failures_mark_the_document_failed_and_a_reimport_retries() {
    // The file exists in the store but the engine has no document for it (e.g. not a real PDF).
    let s = setup(vec![]);
    s.files.add("/in/broken.pdf", b"garbage");

    let outcome = s.ingestion.import(Path::new("/in/broken.pdf")).await;
    let ImportOutcome::Failed {
        id: Some(1),
        reason,
    } = outcome
    else {
        panic!("{outcome:?}")
    };
    assert!(reason.starts_with("Arquivo não encontrado"), "{reason}");
    let row = s.documents.row(1);
    assert_eq!(row.status, DocumentStatus::Failed);
    assert_eq!(row.error.as_deref(), Some(reason.as_str()));

    // Importing the same bytes again retries instead of reporting a duplicate.
    let again = s.ingestion.import(Path::new("/in/broken.pdf")).await;
    assert!(matches!(again, ImportOutcome::Failed { id: Some(1), .. }));
    assert_eq!(s.documents.len(), 1);
}

#[tokio::test]
async fn unreadable_source_files_fail_without_creating_a_document() {
    let s = setup(vec![]);
    let outcome = s.ingestion.import(Path::new("/in/missing.pdf")).await;
    assert!(matches!(outcome, ImportOutcome::Failed { id: None, .. }));
    assert!(s.documents.is_empty());
}

#[tokio::test]
async fn resume_reprocesses_interrupted_documents_only() {
    let s = setup(vec![
        ("/in/a.pdf", b"a", None, vec![text_page(&["A."])]),
        ("/in/b.pdf", b"b", None, vec![text_page(&["B."])]),
    ]);
    s.ingestion.import(Path::new("/in/a.pdf")).await;
    s.ingestion.import(Path::new("/in/b.pdf")).await;
    s.documents.force_status(2, DocumentStatus::Chunking); // the app quit while chunking b.pdf

    let outcomes = s.ingestion.resume().await;
    assert_eq!(
        outcomes,
        [ImportOutcome::Imported {
            id: 2,
            chunks: 1,
            status: DocumentStatus::Embedding
        }]
    );
    assert_eq!(s.documents.row(2).status, DocumentStatus::Embedding);
    assert!(
        s.ingestion.resume().await.is_empty(),
        "nothing left to resume"
    );
}

#[tokio::test]
async fn lists_documents() {
    let s = setup(vec![(
        "/in/a.pdf",
        b"a",
        Some("Contrato A"),
        vec![text_page(&["A."])],
    )]);
    s.ingestion.import(Path::new("/in/a.pdf")).await;
    let docs = s.ingestion.list().await.unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(
        (docs[0].title.as_str(), docs[0].chunk_count),
        ("Contrato A", 1)
    );
}

#[tokio::test]
async fn enqueue_registers_the_document_and_process_indexes_it_later() {
    let s = setup(vec![(
        "/in/a.pdf",
        b"pdf-a",
        Some("A"),
        vec![text_page(&["Carência de 180 dias."])],
    )]);

    let enqueued = s.ingestion.enqueue(Path::new("/in/a.pdf")).await;
    assert_eq!(enqueued, Enqueued::New(1));
    // It is in the library, waiting; nothing was derived from the file yet.
    let row = s.documents.row(1);
    assert_eq!(row.status, DocumentStatus::Queued);
    assert_eq!(row.history, [DocumentStatus::Queued]);
    assert!(row.extraction.is_none() && row.processed.is_none());
    assert_eq!(s.documents.len(), 1);

    let outcome = s.ingestion.process(enqueued).await;
    assert!(matches!(
        outcome,
        ImportOutcome::Imported {
            id: 1,
            chunks: 1,
            ..
        }
    ));
    assert_eq!(s.documents.row(1).status, DocumentStatus::Embedding);

    // The same file again: nothing to enqueue, nothing to process.
    let again = s.ingestion.enqueue(Path::new("/in/a.pdf")).await;
    assert_eq!(again, Enqueued::Duplicate(1));
    assert_eq!(
        s.ingestion.process(again).await,
        ImportOutcome::Duplicate { id: 1 }
    );
}

#[tokio::test]
async fn enqueue_retries_a_failed_document_and_reports_unreadable_files() {
    let s = setup(vec![(
        "/in/a.pdf",
        b"pdf-a",
        None,
        vec![text_page(&["Texto."])],
    )]);
    s.ingestion.enqueue(Path::new("/in/a.pdf")).await;
    s.documents.force_status(1, DocumentStatus::Failed);
    assert_eq!(
        s.ingestion.enqueue(Path::new("/in/a.pdf")).await,
        Enqueued::Retry(1)
    );

    // A document that is still queued (or being read) is already in the queue: not a second run.
    s.documents.force_status(1, DocumentStatus::Chunking);
    assert_eq!(
        s.ingestion.enqueue(Path::new("/in/a.pdf")).await,
        Enqueued::Duplicate(1)
    );

    let missing = s.ingestion.enqueue(Path::new("/in/nao-existe.pdf")).await;
    assert!(matches!(missing, Enqueued::Failed { id: None, .. }));
    assert_eq!(
        s.documents.len(),
        1,
        "nothing is registered for a file that cannot be read"
    );
    assert!(matches!(
        s.ingestion.process(missing).await,
        ImportOutcome::Failed { id: None, .. }
    ));
}
