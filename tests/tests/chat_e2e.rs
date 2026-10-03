//! Chat over the real adapters (PDFium, SQLite, deterministic embedder): conversations saved in
//! SQLite, "explain this document" and "section 3" questions, and the page image of a citation.
//! The `#[ignore]`d tests answer with Apple Foundation Models (`make test-fm`).

use std::{path::Path, sync::Arc};

use nlmx_application::{
    ports::{CancelFlag, LlmProvider},
    services::{
        rag::{RagEngine, RagOptions},
        retrieval::HybridRetriever,
        retriever::Retriever,
    },
    use_cases::{ChatService, DocumentIngestion, EmbedDocuments, ViewDocument},
};
use nlmx_chunker_structural::{HeuristicTokenCounter, StructuralChunker};
use nlmx_domain::{
    chat::MessageStatus,
    ingestion::{ChunkPolicy, ImportOutcome},
};
use nlmx_fs_library::FsLibrary;
use nlmx_llm_fm::FoundationModelsProvider;
use nlmx_pdf_pdfium::PdfiumDocumentEngine;
use nlmx_store_sqlite::Database;
use nlmx_structure_heuristic::HeuristicStructureAnalyzer;
use nlmx_testing::{FakeEmbeddingProvider, FakeLlmProvider, FixedEmbeddingSource};

struct App {
    chat: ChatService,
    pages: ViewDocument,
    report: i64,
}

async fn app(name: &str, llm: Arc<dyn LlmProvider>) -> App {
    let dir = std::env::temp_dir().join(format!("nlmx-chat-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Arc::new(Database::open(dir.join("nlmx.sqlite3")).unwrap());
    let source = FixedEmbeddingSource::of(FakeEmbeddingProvider { dimensions: 64 });
    let engine = Arc::new(PdfiumDocumentEngine::from_default_location().expect("make bootstrap"));
    let ingestion = DocumentIngestion {
        engine: engine.clone(),
        files: Arc::new(FsLibrary::new(dir.join("library"))),
        documents: db.clone(),
        analyzer: Arc::new(HeuristicStructureAnalyzer),
        chunker: Arc::new(StructuralChunker),
        tokens: Arc::new(HeuristicTokenCounter),
        policy: ChunkPolicy::default(),
        embedder: Some(Arc::new(EmbedDocuments {
            embeddings: source.clone(),
            vectors: db.clone(),
            chunks: db.clone(),
            documents: db.clone(),
            batch_size: 16,
        })),
    };
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/adapters/pdf-pdfium/tests/fixtures");
    let ImportOutcome::Imported { id: report, .. } =
        ingestion.import(&fixtures.join("report.pdf")).await
    else {
        panic!("report.pdf")
    };
    ingestion.import(&fixtures.join("text.pdf")).await;
    let retriever = Arc::new(Retriever::new(Arc::new(HybridRetriever::new(
        source,
        db.clone(),
        db.clone(),
        db.clone(),
    ))));
    App {
        chat: ChatService {
            conversations: db.clone(),
            rag: Arc::new(RagEngine::new(retriever, db.clone(), llm)),
            options: RagOptions::default(),
        },
        pages: ViewDocument::new(engine, db),
        report,
    }
}

const EXPLAIN: &str = "Explique este documento.";
const SECTION_3: &str = "Quais são os principais pontos da seção 3?";

#[tokio::test]
async fn explain_and_section_questions_use_the_document_structure() {
    let llm = Arc::new(FakeLlmProvider::available().answering("Resposta [1]."));
    let app = app("fake", llm.clone()).await;

    // Without a chosen document (two in the library) the overview asks for one.
    let all = app.chat.start(None).await.unwrap();
    let (_, a) = app.chat.ask(all.id, EXPLAIN).await.unwrap();
    let m = app
        .chat
        .answer(a, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(m.status, MessageStatus::NotFound);
    assert!(llm.requests().is_empty());

    let c = app.chat.start(Some(app.report)).await.unwrap();
    let (_, a) = app.chat.ask(c.id, EXPLAIN).await.unwrap();
    let m = app
        .chat
        .answer(a, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(m.status, MessageStatus::Answered);
    let sections: Vec<_> = m.sources.iter().filter_map(|s| s.section.clone()).collect();
    assert_eq!(
        sections,
        [
            "1. Introdução",
            "2. Coberturas",
            "3. Prazos",
            "4. Disposições finais"
        ],
        "one passage per section, in order"
    );

    let (_, a) = app.chat.ask(c.id, SECTION_3).await.unwrap();
    let m = app
        .chat
        .answer(a, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(m.status, MessageStatus::Answered);
    assert_eq!(m.sources.len(), 1);
    let s = &m.sources[0];
    assert_eq!(
        (s.section.as_deref(), s.page_start, s.page_end),
        (Some("3. Prazos"), 2, 3)
    );
    assert!(s.cited && s.quote.contains("cento e oitenta dias"));
    assert!(s.bboxes.iter().any(|b| b.page == 2) && s.bboxes.iter().any(|b| b.page == 3));

    // Saved in SQLite, in order, with the conversation titled by its first question.
    let messages = app.chat.messages(c.id).await.unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(
        app.chat.conversation(c.id).await.unwrap().title.as_deref(),
        Some(EXPLAIN)
    );

    // The viewer: outline from the database, the cited page as PNG, its text layer, search.
    let outline = app.pages.outline(app.report).await.unwrap();
    assert_eq!(outline.page_count(), 3);
    assert!(outline.pages[1].width > 500.0 && outline.pages[1].height > 700.0);
    let image = app.pages.render(app.report, 2, 600).await.unwrap();
    assert!(image.png.starts_with(b"\x89PNG"));
    let layer = app.pages.text_layer(app.report, 2).await.unwrap();
    assert!(layer.iter().any(|s| s.text.contains("3. Prazos")));
    let hits = app.pages.search(app.report, "carência").await.unwrap().hits;
    let pages: Vec<u32> = hits.iter().map(|h| h.page).collect();
    assert!(pages.contains(&1) && pages.contains(&2), "{pages:?}");
    for h in &hits {
        let size = &outline.pages[h.page as usize - 1];
        assert!(h.boxes.iter().all(|b| b.left >= 0.0
            && b.right <= size.width
            && b.top >= 0.0
            && b.bottom <= size.height));
    }
    // Page 1 has "carên-" / "cia" (hyphenated at the line end): found too, checked above.
    assert!(
        app.pages
            .search(app.report, "cento e oitenta")
            .await
            .unwrap()
            .hits
            .iter()
            .any(|h| h.page == 3)
    );

    // The section-3 citation opens page 2 with highlights on pages 2 and 3.
    let target = nlmx_domain::viewer::ViewerTarget {
        document_id: s.document_id,
        page: s.page_start,
        highlights: s.bboxes.clone(),
    };
    assert_eq!(target.page, 2);
    assert!(target.anchor_top().is_some());
}

fn real_fm() -> Arc<FoundationModelsProvider> {
    Arc::new(FoundationModelsProvider::system(
        std::env::temp_dir().join(format!("nlmx-chat-fm-{}", std::process::id())),
    ))
}

#[tokio::test]
#[ignore = "runs Apple Foundation Models (make test-fm)"]
async fn real_fm_explains_the_document_and_its_section_3() {
    let fm = real_fm();
    let app = app("real", fm.clone()).await;
    let c = app.chat.start(Some(app.report)).await.unwrap();

    let (_, a) = app.chat.ask(c.id, EXPLAIN).await.unwrap();
    let m = app
        .chat
        .answer(a, &|t| eprint!("{t}"), CancelFlag::default())
        .await
        .unwrap();
    eprintln!(
        "\n— {:?}, cited {:?}",
        m.status,
        m.sources
            .iter()
            .filter(|s| s.cited)
            .map(|s| s.n)
            .collect::<Vec<_>>()
    );
    assert_eq!(m.status, MessageStatus::Answered, "{}", m.content);
    assert!(
        m.sources.iter().any(|s| s.cited),
        "cites passages: {}",
        m.content
    );

    let (_, a) = app.chat.ask(c.id, SECTION_3).await.unwrap();
    let m = app
        .chat
        .answer(a, &|t| eprint!("{t}"), CancelFlag::default())
        .await
        .unwrap();
    eprintln!("\n— {:?}", m.status);
    fm.shutdown().await;
    assert_eq!(m.status, MessageStatus::Answered, "{}", m.content);
    let cited: Vec<_> = m.sources.iter().filter(|s| s.cited).collect();
    assert!(!cited.is_empty(), "{}", m.content);
    assert!(
        cited
            .iter()
            .all(|s| s.section.as_deref() == Some("3. Prazos"))
    );
    let lower = m.content.to_lowercase();
    assert!(
        lower.contains("180") || lower.contains("cento e oitenta"),
        "{}",
        m.content
    );
}
