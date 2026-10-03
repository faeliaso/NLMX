//! The Retriever over real PDFium + SQLite (deterministic embedder): two PDFs with the same text
//! yield one passage, with the other document recorded as a duplicate.

use std::{path::Path, sync::Arc};

use nlmx_application::{
    services::{
        retrieval::HybridRetriever,
        retriever::{Retriever, RetrieverOptions},
    },
    use_cases::{DocumentIngestion, EmbedDocuments},
};
use nlmx_chunker_structural::{HeuristicTokenCounter, StructuralChunker};
use nlmx_domain::ingestion::{ChunkPolicy, DocumentStatus, ImportOutcome};
use nlmx_fs_library::FsLibrary;
use nlmx_pdf_pdfium::PdfiumDocumentEngine;
use nlmx_store_sqlite::Database;
use nlmx_structure_heuristic::HeuristicStructureAnalyzer;
use nlmx_testing::{FakeEmbeddingProvider, FixedEmbeddingSource};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../crates/adapters/pdf-pdfium/tests/fixtures")
        .join(name)
}

#[tokio::test]
async fn duplicated_documents_yield_one_passage_with_citation_data() {
    let dir = std::env::temp_dir().join(format!("nlmx-retriever-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Arc::new(Database::open(dir.join("nlmx.sqlite3")).unwrap());
    let source = FixedEmbeddingSource::of(FakeEmbeddingProvider { dimensions: 64 });
    let embedder = Arc::new(EmbedDocuments {
        embeddings: source.clone(),
        vectors: db.clone(),
        chunks: db.clone(),
        documents: db.clone(),
        batch_size: 16,
    });
    let ingestion = DocumentIngestion {
        engine: Arc::new(PdfiumDocumentEngine::from_default_location().expect("make bootstrap")),
        files: Arc::new(FsLibrary::new(dir.join("library"))),
        documents: db.clone(),
        analyzer: Arc::new(HeuristicStructureAnalyzer),
        chunker: Arc::new(StructuralChunker),
        tokens: Arc::new(HeuristicTokenCounter),
        policy: ChunkPolicy::default(),
        embedder: Some(embedder),
    };
    for name in ["report.pdf", "report-copy.pdf"] {
        let outcome = ingestion.import(&fixture(name)).await;
        assert!(
            matches!(
                outcome,
                ImportOutcome::Imported {
                    status: DocumentStatus::Indexed,
                    ..
                }
            ),
            "{name}: {outcome:?}"
        );
    }

    let retriever = Retriever::new(Arc::new(HybridRetriever::new(
        source,
        db.clone(),
        db.clone(),
        db,
    )));
    let ctx = retriever
        .retrieve(
            "quando termina o prazo de carência",
            &RetrieverOptions {
                min_score: 0.0,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let prazos: Vec<_> = ctx
        .passages
        .iter()
        .filter(|p| p.content.contains("cento e oitenta dias"))
        .collect();
    assert_eq!(
        prazos.len(),
        1,
        "the copy in the second PDF is not repeated"
    );
    let p = prazos[0];
    assert_eq!(
        p.metadata.duplicates.len(),
        1,
        "{:?}",
        p.metadata.duplicates
    );
    assert_ne!(
        p.metadata.duplicates[0].0, p.document_id,
        "duplicate lives in the other document"
    );

    // Fields for the RAG and for citations.
    assert_eq!(p.page, 2);
    assert_eq!((p.source.page_start, p.source.page_end), (2, 3));
    assert_eq!(p.source.section.as_deref(), Some("3. Prazos"));
    assert!(
        p.source.label.ends_with("pp. 2–3 · 3. Prazos"),
        "{}",
        p.source.label
    );
    let pages: Vec<u32> = p.metadata.bboxes.iter().map(|b| b.page).collect();
    assert!(
        pages.contains(&2) && pages.contains(&3),
        "highlight boxes on both pages: {pages:?}"
    );
    assert!(p.metadata.matched_by.semantic && p.metadata.matched_by.lexical);
    assert!((0.0..=1.0).contains(&p.score));
    assert_eq!(
        ctx.passages[0].chunk_id, p.chunk_id,
        "the most relevant passage comes first"
    );

    // No passage text appears twice in the context.
    let mut contents: Vec<&str> = ctx.passages.iter().map(|p| p.content.as_str()).collect();
    contents.sort();
    contents.dedup();
    assert_eq!(contents.len(), ctx.passages.len());
}
