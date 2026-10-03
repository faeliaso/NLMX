//! ViewDocument: outline from the database, text layer and search over cached spans.

use std::{path::Path, sync::Arc};

use nlmx_application::use_cases::{DocumentIngestion, PageError, ViewDocument};
use nlmx_domain::document::{BoundingBox, DocumentMetadata, TextSpan};
use nlmx_testing::{
    FakeChunker, FakeDocumentEngine, FakeDocumentRepository, FakeFileStore, FakePage,
    FakeStructureAnalyzer, WordTokenCounter,
};

fn span(text: &str, top: f32) -> TextSpan {
    TextSpan {
        text: text.into(),
        bbox: BoundingBox {
            left: 72.0,
            top,
            right: 72.0 + text.chars().count() as f32 * 6.0,
            bottom: top + 12.0,
        },
        font_name: "Helvetica".into(),
        font_size: 11.0,
        bold: false,
        italic: false,
    }
}

async fn viewer() -> (ViewDocument, i64) {
    let files = FakeFileStore::default();
    let sha = files.add("/in/r.pdf", b"pdf");
    let engine = Arc::new(FakeDocumentEngine::default().with_document(
        FakeFileStore::library_path(&sha),
        DocumentMetadata {
            title: Some("Relatório".into()),
            ..Default::default()
        },
        vec![
            FakePage {
                spans: vec![span("Introdução sobre a carência", 100.0)],
                images: vec![],
            },
            FakePage {
                spans: vec![span("3. Prazos", 80.0), span("A CARÊNCIA termina", 100.0)],
                images: vec![],
            },
            FakePage {
                spans: vec![span("Fim.", 80.0)],
                images: vec![],
            },
        ],
    ));
    let documents = Arc::new(FakeDocumentRepository::default());
    let ingestion = DocumentIngestion {
        engine: engine.clone(),
        files: Arc::new(files),
        documents: documents.clone(),
        analyzer: Arc::new(FakeStructureAnalyzer),
        chunker: Arc::new(FakeChunker),
        tokens: Arc::new(WordTokenCounter),
        policy: Default::default(),
        embedder: None,
    };
    let nlmx_domain::ingestion::ImportOutcome::Imported { id, .. } =
        ingestion.import(Path::new("/in/r.pdf")).await
    else {
        panic!("import")
    };
    (ViewDocument::new(engine, documents), id)
}

#[tokio::test]
async fn outline_lists_pages_with_their_sizes() {
    let (viewer, id) = viewer().await;
    let outline = viewer.outline(id).await.unwrap();
    assert_eq!(outline.page_count(), 3);
    assert_eq!(
        (
            outline.pages[1].number,
            outline.pages[1].width,
            outline.pages[1].height
        ),
        (2, 612.0, 792.0)
    );
    assert_eq!(viewer.outline(99).await.unwrap_err(), PageError::NotFound);
}

#[tokio::test]
async fn text_layer_and_search() {
    let (viewer, id) = viewer().await;
    let layer = viewer.text_layer(id, 2).await.unwrap();
    assert_eq!(
        layer.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
        ["3. Prazos", "A CARÊNCIA termina"]
    );
    assert_eq!(
        viewer.text_layer(id, 9).await.unwrap_err(),
        PageError::NotFound
    );

    let results = viewer.search(id, "carencia").await.unwrap();
    let pages: Vec<u32> = results.hits.iter().map(|h| h.page).collect();
    assert_eq!(pages, [1, 2]);
    let b = results.hits[1].boxes[0];
    assert_eq!(
        (b.left, b.top),
        (72.0 + 2.0 * 6.0, 100.0),
        "the word, not the whole line"
    );
    assert!(!results.truncated);
    assert!(viewer.search(id, "  ").await.unwrap().hits.is_empty());
}

#[tokio::test]
async fn renders_pages_within_range() {
    let (viewer, id) = viewer().await;
    assert!(viewer.render(id, 1, 900).await.is_ok());
    assert_eq!(
        viewer.render(id, 4, 900).await.unwrap_err(),
        PageError::NotFound
    );
}
