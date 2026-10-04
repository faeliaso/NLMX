//! The app flow with real PDFium + SQLite and a deterministic embedder:
//! import → chunks → automatic embeddings → `indexed` → hybrid search.

use std::{path::Path, sync::Arc};

use nlmx_application::{
    services::retrieval::{HybridRetriever, RetrievalMode},
    use_cases::{DocumentIngestion, EmbedDocuments},
};
use nlmx_chunker_structural::{HeuristicTokenCounter, StructuralChunker};
use nlmx_domain::{
    ingestion::{ChunkPolicy, DocumentStatus, ImportOutcome},
    retrieval::{RetrievalFilter, RetrievalOptions},
};
use nlmx_fs_library::FsLibrary;
use nlmx_pdf_pdfium::PdfiumDocumentEngine;
use nlmx_store_sqlite::Database;
use nlmx_structure_heuristic::HeuristicStructureAnalyzer;
use nlmx_testing::{FakeEmbeddingProvider, FixedEmbeddingSource};

#[tokio::test]
async fn imported_pdfs_are_embedded_indexed_and_searchable() {
    let dir = std::env::temp_dir().join(format!("nlmx-app-search-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Arc::new(Database::open(dir.join("nlmx.sqlite3")).unwrap());
    let source = FixedEmbeddingSource::of(FakeEmbeddingProvider { dimensions: 64 });

    let embedder = Arc::new(EmbedDocuments {
        progress: None,
        embeddings: source.clone(),
        vectors: db.clone(),
        chunks: db.clone(),
        documents: db.clone(),
        batch_size: 16,
    });
    let ingestion = DocumentIngestion {
        pipeline: None,
        progress: None,
        viewer: None,
        engine: Arc::new(PdfiumDocumentEngine::from_default_location().expect("make bootstrap")),
        files: Arc::new(FsLibrary::new(dir.join("library"))),
        documents: db.clone(),
        analyzer: Arc::new(HeuristicStructureAnalyzer),
        chunker: Arc::new(StructuralChunker),
        tokens: Arc::new(HeuristicTokenCounter),
        policy: ChunkPolicy::default(),
        embedder: Some(embedder),
    };
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../crates/adapters/pdf-pdfium/tests/fixtures/report.pdf");
    let outcome = ingestion.import(&fixture).await;
    assert!(
        matches!(
            outcome,
            ImportOutcome::Imported {
                status: DocumentStatus::Indexed,
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(
        ingestion.list().await.unwrap()[0].status,
        DocumentStatus::Indexed
    );

    let retriever = HybridRetriever::new(source, db.clone(), db.clone(), db.clone());
    let result = retriever
        .retrieve(
            "quando termina o prazo de carência",
            &RetrievalOptions {
                top_k: 3,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(result.mode, RetrievalMode::Hybrid);
    let top = &result.chunks[0];
    assert_eq!(top.chunk.section.as_deref(), Some("3. Prazos"));
    assert_eq!((top.chunk.page_start, top.chunk.page_end), (2, 3));
    assert_eq!(top.chunk.document_title, "Relatório de Coberturas");
    assert!(top.semantic.is_some() && top.lexical.is_some());

    // Page filter: only page 3 and later.
    let page3 = RetrievalOptions {
        filter: RetrievalFilter {
            pages: Some(nlmx_domain::retrieval::PageRange { from: 3, to: 3 }),
            ..Default::default()
        },
        ..Default::default()
    };
    let result = retriever.retrieve("operadora", &page3).await.unwrap();
    assert!(result.chunks.iter().all(|c| c.chunk.page_end >= 3));
    assert_eq!(
        result.chunks[0].chunk.section.as_deref(),
        Some("4. Disposições finais")
    );
}
