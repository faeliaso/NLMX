//! Ingest a PDF, embed its chunks with the real model, index them and search (KNN).
//! Ignored by default; run with `make test-llama` (needs runtime/llama and the Qwen3 model).

use std::{path::Path, sync::Arc};

use nlmx_application::{
    ports::{EmbeddingProvider, VectorStore},
    use_cases::DocumentIngestion,
};
use nlmx_chunker_structural::{HeuristicTokenCounter, StructuralChunker};
use nlmx_domain::{
    embedding::EmbeddingPurpose,
    ingestion::{ChunkPolicy, ImportOutcome},
    vectors::{EmbeddingSpace, VectorFilter},
};
use nlmx_embed_llama::{EmbeddingConfig, llama_server_path, provider};
use nlmx_fs_library::FsLibrary;
use nlmx_pdf_pdfium::PdfiumDocumentEngine;
use nlmx_store_sqlite::Database;
use nlmx_structure_heuristic::HeuristicStructureAnalyzer;

#[tokio::test]
#[ignore = "needs runtime/llama and models/Qwen3-Embedding-0.6B-Q8_0.gguf"]
async fn semantic_search_over_an_ingested_pdf() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let dir = std::env::temp_dir().join(format!("nlmx-vector-real-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let db = Database::open(dir.join("nlmx.sqlite3")).unwrap();
    let ingestion = DocumentIngestion {
        pipeline: None,
        progress: None,
        viewer: None,
        engine: Arc::new(PdfiumDocumentEngine::from_default_location().unwrap()),
        files: Arc::new(FsLibrary::new(dir.join("library"))),
        documents: Arc::new(db.clone()),
        analyzer: Arc::new(HeuristicStructureAnalyzer),
        chunker: Arc::new(StructuralChunker),
        tokens: Arc::new(HeuristicTokenCounter),
        policy: ChunkPolicy::default(),
        embedder: None,
    };
    let fixture = root.join("crates/adapters/pdf-pdfium/tests/fixtures/report.pdf");
    let ImportOutcome::Imported { id: document, .. } = ingestion.import(&fixture).await else {
        panic!("import")
    };

    // Chunks as stored (id, section, text).
    let conn = rusqlite::Connection::open(dir.join("nlmx.sqlite3")).unwrap();
    let mut stmt = conn.prepare("SELECT id, section_path, text FROM document_chunks WHERE document_id = ?1 ORDER BY ordinal").unwrap();
    let chunks: Vec<(i64, String, String)> = stmt
        .query_map([document], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    let config = EmbeddingConfig::load(&root.join("models/embedding.example.json"))
        .expect("fetch-embedding-model.sh");
    let embeddings = provider(config, llama_server_path().expect("make bootstrap"), &dir);
    // The dimension comes from the loaded model, not from code.
    let space = EmbeddingSpace::from_identity(&embeddings.identity().await.unwrap());
    let index = db.create_index(&space).await.unwrap();
    assert_eq!(index.space.dimensions, 1024);

    let texts: Vec<String> = chunks.iter().map(|(_, _, t)| t.clone()).collect();
    let vectors = embeddings
        .embed_batch(&texts, EmbeddingPurpose::Passage)
        .await
        .unwrap();
    let items: Vec<(i64, Vec<f32>)> = chunks.iter().map(|(id, _, _)| *id).zip(vectors).collect();
    db.insert_batch(index.id, &items).await.unwrap();
    assert_eq!(db.count(index.id).await.unwrap(), chunks.len() as u64);

    let section_of = |chunk: i64| {
        chunks
            .iter()
            .find(|(id, _, _)| *id == chunk)
            .map(|(_, s, _)| s.clone())
            .unwrap()
    };
    for (question, expected) in [
        ("Quais exames e consultas estão cobertos?", "2. Coberturas"),
        ("Em quantos dias termina o prazo de carência?", "3. Prazos"),
        (
            "Who resolves cases not covered by the rules?",
            "4. Disposições finais",
        ),
    ] {
        let q = embeddings
            .embed(question, EmbeddingPurpose::Query)
            .await
            .unwrap();
        let hits = db
            .search(index.id, &q, 3, &VectorFilter::default())
            .await
            .unwrap();
        let ranked: Vec<(String, f32)> = hits
            .iter()
            .map(|h| (section_of(h.chunk_id), h.similarity))
            .collect();
        eprintln!("{question:50} → {ranked:.3?}");
        assert_eq!(ranked[0].0, expected, "{question}");
    }
    embeddings.shutdown().await;
}
