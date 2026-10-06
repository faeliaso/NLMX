//! RemoveDocument with in-memory fakes: what goes, what stays, and when removal is refused.

use std::{path::Path, sync::Arc, sync::atomic::Ordering};

use nlmx_application::{
    ports::DocumentRepository,
    use_cases::{DocumentIngestion, PageError, RemoveDocument, RemoveError, ViewDocument},
};
use nlmx_domain::{
    document::{BoundingBox, DocumentMetadata, TextSpan},
    ingestion::{DocumentStatus, ImportOutcome, RemovalImpact},
};
use nlmx_testing::{
    FakeChunker, FakeDocumentEngine, FakeDocumentRepository, FakeFileStore, FakePage,
    FakeStructureAnalyzer, WordTokenCounter,
};

fn page(text: &str) -> FakePage {
    FakePage {
        spans: vec![TextSpan {
            text: text.into(),
            bbox: BoundingBox {
                left: 72.0,
                top: 100.0,
                right: 300.0,
                bottom: 112.0,
            },
            font_name: "Helvetica".into(),
            font_size: 11.0,
            bold: false,
            italic: false,
        }],
        images: vec![],
    }
}

struct Setup {
    remover: RemoveDocument,
    viewer: Arc<ViewDocument>,
    documents: Arc<FakeDocumentRepository>,
    files: Arc<FakeFileStore>,
    id: i64,
    sha: String,
}

async fn setup() -> Setup {
    let files = Arc::new(FakeFileStore::default());
    let sha = files.add("/in/contrato.pdf", b"pdf");
    let engine = Arc::new(FakeDocumentEngine::default().with_document(
        FakeFileStore::library_path(&sha),
        DocumentMetadata::default(),
        vec![page("A carência é de 180 dias."), page("Reajuste anual.")],
    ));
    let documents = Arc::new(FakeDocumentRepository::default());
    let ingestion = DocumentIngestion {
        pipeline: None,
        progress: None,
        viewer: None,
        engine: engine.clone(),
        files: files.clone(),
        documents: documents.clone(),
        analyzer: Arc::new(FakeStructureAnalyzer),
        chunker: Arc::new(FakeChunker),
        tokens: Arc::new(WordTokenCounter),
        policy: Default::default(),
        embedder: None,
    };
    let ImportOutcome::Imported { id, .. } = ingestion.import(Path::new("/in/contrato.pdf")).await
    else {
        panic!("import")
    };
    let viewer = Arc::new(ViewDocument::new(engine, documents.clone()));
    Setup {
        remover: RemoveDocument {
            documents: documents.clone(),
            files: files.clone(),
            viewer: Some(viewer.clone()),
        },
        viewer,
        documents,
        files,
        id,
        sha,
    }
}

#[tokio::test]
async fn removes_the_document_its_file_and_its_cached_text() {
    let s = setup().await;
    // Cached by the viewer before the removal.
    assert!(!s.viewer.text_layer(s.id, 1).await.unwrap().is_empty());

    let impact = s.remover.impact(s.id).await.unwrap();
    assert!(impact.chunks > 0);
    assert_eq!(s.remover.remove(s.id).await.unwrap(), impact);

    assert!(s.documents.get(s.id).await.unwrap().is_none());
    assert!(s.documents.list().await.unwrap().is_empty());
    assert_eq!(s.files.removed(), std::slice::from_ref(&s.sha));
    assert_eq!(
        s.viewer.text_layer(s.id, 1).await.unwrap_err(),
        PageError::NotFound,
        "no text served from the cache"
    );
    assert_eq!(s.remover.remove(s.id).await, Err(RemoveError::NotFound));
}

#[tokio::test]
async fn refuses_while_the_document_is_being_read() {
    let s = setup().await;
    for status in [
        DocumentStatus::Queued,
        DocumentStatus::Extracting,
        DocumentStatus::Structuring,
        DocumentStatus::Chunking,
    ] {
        s.documents.force_status(s.id, status);
        assert_eq!(
            s.remover.remove(s.id).await,
            Err(RemoveError::Busy),
            "{status}"
        );
        assert_eq!(s.remover.impact(s.id).await, Err(RemoveError::Busy));
    }
    assert!(s.files.removed().is_empty());

    // Waiting for embeddings (possibly forever, without a model) can be removed.
    s.documents.force_status(s.id, DocumentStatus::Embedding);
    assert!(s.remover.remove(s.id).await.is_ok());
}

#[tokio::test]
async fn a_file_that_cannot_be_deleted_does_not_undo_the_removal() {
    let s = setup().await;
    s.files.fail_remove.store(true, Ordering::SeqCst);
    assert!(s.remover.remove(s.id).await.is_ok());
    assert!(s.documents.get(s.id).await.unwrap().is_none());

    // At the next start, the library keeps only files of existing documents.
    assert_eq!(s.remover.prune_library().await.unwrap(), 0);
    assert_eq!(s.files.pruned_keeping(), Some(vec![]));
}

#[tokio::test]
async fn unknown_documents_are_reported() {
    let s = setup().await;
    assert_eq!(s.remover.impact(99).await, Err(RemoveError::NotFound));
    assert_eq!(s.remover.remove(99).await, Err(RemoveError::NotFound));
    assert_eq!(
        s.remover.impact(s.id).await.unwrap(),
        RemovalImpact {
            chunks: s.remover.impact(s.id).await.unwrap().chunks,
            ..Default::default()
        },
        "the fake has no chat history"
    );
}
