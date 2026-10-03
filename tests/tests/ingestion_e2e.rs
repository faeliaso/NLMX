//! The ingestion pipeline with every real adapter: PDFium, the heuristic analyzer, the structural
//! chunker, the file library and SQLite — on the fixture PDFs of `pdf-pdfium`.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use nlmx_application::use_cases::DocumentIngestion;
use nlmx_chunker_structural::{HeuristicTokenCounter, StructuralChunker, sha256_hex};
use nlmx_domain::ingestion::{ChunkPolicy, DocumentStatus, ImportOutcome};
use nlmx_fs_library::FsLibrary;
use nlmx_pdf_pdfium::PdfiumDocumentEngine;
use nlmx_store_sqlite::Database;
use nlmx_structure_heuristic::HeuristicStructureAnalyzer;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../crates/adapters/pdf-pdfium/tests/fixtures")
        .join(name)
}

struct App {
    ingestion: DocumentIngestion,
    db_path: PathBuf,
    library: PathBuf,
}

fn app(name: &str) -> App {
    let dir = std::env::temp_dir().join(format!("nlmx-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("nlmx.sqlite3");
    let library = dir.join("library");
    let engine =
        PdfiumDocumentEngine::from_default_location().expect("PDFium — run `make bootstrap`");
    let ingestion = DocumentIngestion {
        engine: Arc::new(engine),
        files: Arc::new(FsLibrary::new(&library)),
        documents: Arc::new(Database::open(&db_path).unwrap()),
        analyzer: Arc::new(HeuristicStructureAnalyzer),
        chunker: Arc::new(StructuralChunker),
        tokens: Arc::new(HeuristicTokenCounter),
        policy: ChunkPolicy::default(),
        embedder: None,
    };
    App {
        ingestion,
        db_path,
        library,
    }
}

#[derive(Debug)]
struct ChunkRow {
    document_id: i64,
    ordinal: i64,
    page_start: i64,
    page_end: i64,
    section: Option<String>,
    text: String,
    token_count: i64,
    bboxes: String,
    content_hash: String,
}

fn chunks(db: &Path) -> Vec<ChunkRow> {
    let conn = rusqlite::Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT document_id, ordinal, page_start, page_end, section_path, text, token_count, bboxes, content_hash
             FROM document_chunks ORDER BY document_id, ordinal",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok(ChunkRow {
            document_id: r.get(0)?,
            ordinal: r.get(1)?,
            page_start: r.get(2)?,
            page_end: r.get(3)?,
            section: r.get(4)?,
            text: r.get(5)?,
            token_count: r.get(6)?,
            bboxes: r.get(7)?,
            content_hash: r.get(8)?,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[tokio::test]
async fn report_pdf_becomes_structured_chunks() {
    let app = app("report");
    let outcome = app.ingestion.import(&fixture("report.pdf")).await;
    let ImportOutcome::Imported {
        id,
        chunks: n,
        status,
    } = outcome
    else {
        panic!("{outcome:?}")
    };
    assert_eq!(status, DocumentStatus::Embedding);

    let rows = chunks(&app.db_path);
    assert_eq!(rows.len() as u32, n);
    let summary: Vec<(i64, Option<&str>, &str)> = rows
        .iter()
        .map(|c| (c.page_start, c.section.as_deref(), c.text.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (
                1,
                Some("1. Introdução"),
                "Este relatório descreve o período de carência aplicado aos contratos e as regras de cobertura vigentes."
            ),
            (
                1,
                Some("2. Coberturas"),
                "• Consultas ambulatoriais\n\n• Exames laboratoriais\n\n• Internação hospitalar"
            ),
            (
                2,
                Some("3. Prazos"),
                "Os prazos contam a partir da assinatura e o prazo de carência termina após cento e oitenta dias corridos."
            ),
            (
                3,
                Some("4. Disposições finais"),
                "Casos omissos serão resolvidos pela operadora."
            ),
        ]
    );

    for (i, c) in rows.iter().enumerate() {
        assert_eq!(
            (c.document_id, c.ordinal),
            (id, i as i64),
            "chunk_index is sequential"
        );
        assert!(c.token_count > 0);
        assert_eq!(c.content_hash, sha256_hex(&c.text));
        assert!(c.bboxes.starts_with(r#"[{"page":"#), "{}", c.bboxes);
        assert!(
            !c.text.contains("Relatório") || c.text.contains("relatório descreve"),
            "header removed: {}",
            c.text
        );
        assert!(!c.text.contains("Página"), "footer removed: {}", c.text);
    }
    // The paragraph that continues onto page 3 keeps both pages and a box on each.
    let prazos = &rows[2];
    assert_eq!((prazos.page_start, prazos.page_end), (2, 3));
    assert!(
        prazos.bboxes.contains(r#""page":2"#) && prazos.bboxes.contains(r#""page":3"#),
        "{}",
        prazos.bboxes
    );

    let docs = app.ingestion.list().await.unwrap();
    assert_eq!(docs[0].title, "Relatório de Coberturas");
    assert_eq!((docs[0].page_count, docs[0].chunk_count), (Some(3), n));
}

#[tokio::test]
async fn reimporting_is_detected_and_reingesting_is_idempotent() {
    let app = app("idempotent");
    let ImportOutcome::Imported { id, .. } = app.ingestion.import(&fixture("report.pdf")).await
    else {
        panic!()
    };
    let first = chunks(&app.db_path);

    // Same bytes under another name: duplicate by hash, nothing changes.
    let copy = app.library.parent().unwrap().join("copia-do-relatorio.pdf");
    std::fs::copy(fixture("report.pdf"), &copy).unwrap();
    assert_eq!(
        app.ingestion.import(&copy).await,
        ImportOutcome::Duplicate { id }
    );
    assert_eq!(
        std::fs::read_dir(&app.library).unwrap().count(),
        1,
        "one library file"
    );

    // Running the pipeline again yields the same chunks (text, pages, sections, hashes).
    assert!(matches!(
        app.ingestion.ingest(id).await,
        ImportOutcome::Imported { .. }
    ));
    let second = chunks(&app.db_path);
    let key = |c: &ChunkRow| {
        (
            c.ordinal,
            c.page_start,
            c.section.clone(),
            c.text.clone(),
            c.content_hash.clone(),
            c.bboxes.clone(),
        )
    };
    assert_eq!(
        first.iter().map(key).collect::<Vec<_>>(),
        second.iter().map(key).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn pages_without_text_are_recorded_and_scans_need_ocr() {
    let app = app("pages");
    let ImportOutcome::Imported { id, status, .. } =
        app.ingestion.import(&fixture("mixed.pdf")).await
    else {
        panic!()
    };
    assert_eq!(status, DocumentStatus::Embedding);
    let conn = rusqlite::Connection::open(&app.db_path).unwrap();
    let mut stmt = conn.prepare("SELECT page_number, has_text FROM document_pages WHERE document_id = ?1 ORDER BY page_number").unwrap();
    let pages: Vec<(i64, bool)> = stmt
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(pages, [(1, true), (2, false), (3, false)]);

    let scan = app.ingestion.import(&fixture("scanned.pdf")).await;
    assert!(
        matches!(
            scan,
            ImportOutcome::Imported {
                chunks: 0,
                status: DocumentStatus::NeedsOcr,
                ..
            }
        ),
        "{scan:?}"
    );
}

#[tokio::test]
async fn invalid_pdfs_fail_with_a_reason() {
    let app = app("invalid");
    let outcome = app.ingestion.import(&fixture("corrupt.pdf")).await;
    let ImportOutcome::Failed {
        id: Some(_),
        reason,
    } = outcome
    else {
        panic!("{outcome:?}")
    };
    assert!(reason.contains("PDF válido"), "{reason}");
    let docs = app.ingestion.list().await.unwrap();
    assert_eq!(docs[0].status, DocumentStatus::Failed);
}
