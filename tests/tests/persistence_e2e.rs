//! Documents of every format through the real pipeline into the real database: what is saved
//! is what is read back, saving again changes nothing, and removing leaves nothing.

mod support;

use std::{path::PathBuf, sync::Arc};

use nlmx_application::{
    ports::{DocumentRepository, DocumentSource, InsertOutcome, LexicalIndex, NewDocument},
    services::{
        parsing::{ParserRegistry, PdfDocumentParser},
        pipeline::ContentPipeline,
    },
};
use nlmx_chunker_structural::{HeuristicTokenCounter, MultiFormatChunker};
use nlmx_domain::{
    document_type::DocumentType,
    ingestion::{ChunkPolicy, DocumentStatus},
    retrieval::{LexicalQuery, RetrievalFilter},
    source::SourceLocation,
};
use nlmx_normalizer_text::TextNormalizer;
use nlmx_parser_epub::EpubDocumentParser;
use nlmx_parser_text::{CsvDocumentParser, MarkdownDocumentParser, TextDocumentParser};
use nlmx_pdf_pdfium::PdfiumDocumentEngine;
use nlmx_store_sqlite::Database;
use nlmx_structure_heuristic::HeuristicStructureAnalyzer;
use sha2::{Digest, Sha256};
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

fn pipeline() -> ContentPipeline {
    ContentPipeline {
        parsers: ParserRegistry::new()
            .with(Arc::new(PdfDocumentParser::new(
                engine(),
                Arc::new(HeuristicStructureAnalyzer),
            )))
            .with(Arc::new(MarkdownDocumentParser))
            .with(Arc::new(TextDocumentParser))
            .with(Arc::new(CsvDocumentParser))
            .with(Arc::new(EpubDocumentParser::default())),
        normalizer: Arc::new(TextNormalizer),
        chunker: Arc::new(MultiFormatChunker),
        tokens: Arc::new(HeuristicTokenCounter),
        policy: ChunkPolicy::default(),
    }
}

fn text_fixture(name: &str) -> PathBuf {
    root()
        .join("crates/adapters/parser-text/tests/fixtures")
        .join(name)
}

fn cases() -> Vec<(PathBuf, DocumentType)> {
    vec![
        (fixture("report.pdf"), DocumentType::Pdf),
        (text_fixture("guia.md"), DocumentType::Markdown),
        (text_fixture("notas.txt"), DocumentType::Text),
        (text_fixture("pessoas.csv"), DocumentType::Csv),
        (
            root().join("crates/adapters/parser-epub/tests/fixtures/livro.epub"),
            DocumentType::Epub,
        ),
    ]
}

fn sha256(path: &PathBuf) -> String {
    Sha256::digest(std::fs::read(path).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn counts(conn: &rusqlite::Connection) -> [i64; 4] {
    let n = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    [
        n("SELECT count(*) FROM documents"),
        n("SELECT count(*) FROM document_chunks"),
        n("SELECT count(*) FROM document_sections"),
        n("SELECT count(*) FROM chunk_provenance"),
    ]
}

#[tokio::test]
async fn every_format_is_saved_and_read_back_as_the_pipeline_made_it() {
    let dir = temp_dir("persistence");
    let path = dir.join("nlmx.sqlite3");
    let db = Database::open(&path).unwrap();
    let pipeline = pipeline();

    let mut saved = Vec::new();
    for (file, kind) in cases() {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        let sha = sha256(&file);
        let InsertOutcome::Inserted(id) = db
            .insert(NewDocument {
                sha256: sha.clone(),
                original_filename: name.clone(),
                original_path: file.to_string_lossy().to_string(),
                library_path: format!("/biblioteca/{sha}"),
                file_size: std::fs::metadata(&file).unwrap().len(),
                document_type: kind,
            })
            .await
            .unwrap()
        else {
            panic!("{name}: a new document");
        };

        let processed = pipeline
            .run(&DocumentSource::from_path(&file), id, Some(&name))
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let title = processed
            .metadata
            .title
            .clone()
            .unwrap_or_else(|| name.clone());
        let stored = processed.clone().into_stored(title.clone());
        db.save_processed(id, stored).await.unwrap();

        // The document row says what it is.
        let record = db.get(id).await.unwrap().unwrap();
        assert_eq!(record.document_type, kind, "{name}");
        assert_eq!(record.previewable(), kind == DocumentType::Pdf, "{name}");
        assert_eq!(record.status, DocumentStatus::Embedding, "{name}");

        // Chunks come back with the same text, path, location, metadata and hash.
        let read = db.chunks_of(id).await.unwrap();
        assert_eq!(read.len(), processed.chunks.len(), "{name}");
        for (got, expected) in read.iter().zip(&processed.chunks) {
            assert_eq!(got.text, expected.text, "{name}");
            assert_eq!(got.section_path, expected.section_path, "{name}");
            assert_eq!(got.content_hash, expected.content_hash, "{name}");
            assert_eq!(got.index, expected.index, "{name}");
            assert_eq!(got.metadata, expected.metadata, "{name}");
            assert_eq!(got.token_count, expected.token_count, "{name}");
            match (&got.location, &expected.location) {
                // PDF boxes are stored with two decimals.
                (
                    SourceLocation::Pdf {
                        page_start: a,
                        page_end: b,
                        boxes: ab,
                    },
                    SourceLocation::Pdf {
                        page_start: c,
                        page_end: d,
                        boxes: cd,
                    },
                ) => {
                    assert_eq!((a, b), (c, d), "{name}");
                    assert_eq!(ab.len(), cd.len(), "{name}");
                    for (x, y) in ab.iter().zip(cd) {
                        assert_eq!(x.page, y.page);
                        assert!((x.bbox.left - y.bbox.left).abs() < 0.01);
                        assert!((x.bbox.bottom - y.bbox.bottom).abs() < 0.01);
                    }
                }
                (a, b) => assert_eq!(a, b, "{name}"),
            }
        }
        // The structure comes back too.
        let sections = db.sections_of(id).await.unwrap();
        assert_eq!(sections.len(), processed.outline.len(), "{name}");
        for (got, expected) in sections.iter().zip(&processed.outline) {
            assert_eq!(
                (got.kind, &got.title, got.level),
                (expected.kind, &expected.title, expected.level)
            );
            assert_eq!(got.path, expected.path);
            assert_eq!(got.location, expected.location);
        }
        saved.push((id, name, processed.chunks.len()));
    }

    // Saving everything again changes nothing.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let before = counts(&conn);
    assert_eq!(before[0], 5);
    for ((file, _), (id, _, _)) in cases().iter().zip(&saved) {
        let processed = pipeline
            .run(&DocumentSource::from_path(file), *id, None)
            .await
            .unwrap();
        db.save_processed(*id, processed.into_stored("t".into()))
            .await
            .unwrap();
    }
    assert_eq!(counts(&conn), before);

    // The existing lexical search finds text of a format that is not a PDF.
    let hits = LexicalIndex::search(
        &db,
        &LexicalQuery::from_query("marcador"),
        10,
        &RetrievalFilter::default(),
    )
    .await
    .unwrap();
    let markdown = saved.iter().find(|(_, n, _)| n == "guia.md").unwrap().0;
    assert!(hits.iter().any(|h| h.document_id == markdown));

    // Removing a document takes its structure and provenance with it, and only its own.
    for (id, name, _) in &saved {
        db.remove(*id)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{name}"));
    }
    assert_eq!(counts(&conn), [0, 0, 0, 0]);
    let _ = std::fs::remove_dir_all(dir);
}
