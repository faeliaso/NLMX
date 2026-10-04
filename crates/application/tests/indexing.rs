//! `Indexing`: the report of the Indexação section, its exclusive actions and retries.

use std::sync::Arc;

use nlmx_application::{
    ports::{DocumentRepository, NewDocument},
    use_cases::{
        EmbedDocuments, EmbedOutcome, Indexing, IndexingActivity, IndexingError, RetryOutcome,
    },
};
use nlmx_domain::ingestion::DocumentStatus;
use nlmx_testing::{
    FakeCorpus, FakeDocumentRepository, FakeEmbeddingProvider, FakeVectorStore,
    FixedEmbeddingSource,
};

async fn indexing(source: Arc<FixedEmbeddingSource>) -> (Indexing, Arc<FakeDocumentRepository>) {
    let documents = Arc::new(FakeDocumentRepository::default());
    for sha in ['a', 'b', 'c'] {
        documents
            .insert(NewDocument {
                sha256: sha.to_string().repeat(64),
                original_filename: format!("{sha}.pdf"),
                original_path: String::new(),
                library_path: String::new(),
                file_size: 1,
                document_type: nlmx_domain::document_type::DocumentType::Pdf,
            })
            .await
            .unwrap();
    }
    documents
        .set_status(1, DocumentStatus::Failed, Some("PDF inválido".into()))
        .await
        .unwrap();
    documents.force_status(2, DocumentStatus::Embedding);
    documents.force_status(3, DocumentStatus::Indexed);
    let embedder = Arc::new(EmbedDocuments {
        progress: None,
        embeddings: source.clone(),
        vectors: Arc::new(FakeVectorStore::default().with_chunk(1, 2)),
        chunks: Arc::new(FakeCorpus::default().chunk(1, 2, 1, "carência de 180 dias")),
        documents: documents.clone(),
        batch_size: 8,
    });
    let indexing = Indexing {
        reader: documents.clone(),
        documents: documents.clone(),
        embeddings: source,
        embedder,
        ingestion: Err("Motor de PDF indisponível".into()),
        activity: Arc::new(IndexingActivity::default()),
    };
    (indexing, documents)
}

#[tokio::test]
async fn actions_are_exclusive_but_imports_may_overlap() {
    let activity = Arc::new(IndexingActivity::default());
    assert!(!activity.is_running());
    let import = activity.begin();
    let other_import = activity.begin();
    assert!(activity.try_begin().is_none(), "busy while importing");
    drop((import, other_import));
    let action = activity.try_begin().expect("free again");
    assert!(activity.try_begin().is_none(), "one action at a time");
    assert!(activity.is_running());
    drop(action);
    assert!(!activity.is_running());
}

#[tokio::test]
async fn the_report_names_the_model_and_whether_work_is_running() {
    let (indexing, _) = indexing(FixedEmbeddingSource::of(FakeEmbeddingProvider {
        dimensions: 4,
    }))
    .await;
    let report = indexing.report().await.unwrap();
    assert!(report.model_id.is_some());
    assert_eq!(report.snapshot.jobs.len(), 3);
    assert!(!report.active());

    let _guard = indexing.begin().unwrap();
    assert!(indexing.report().await.unwrap().active());
    assert_eq!(indexing.begin().unwrap_err(), IndexingError::Busy);
}

#[tokio::test]
async fn retries_depend_on_where_the_document_stopped() {
    let (indexing, documents) = indexing(FixedEmbeddingSource::of(FakeEmbeddingProvider {
        dimensions: 4,
    }))
    .await;
    // Re-reading a failed document needs the PDF engine.
    assert_eq!(
        indexing.retry(1).await,
        Err(IndexingError::Unavailable(
            "Motor de PDF indisponível".into()
        ))
    );
    assert_eq!(
        indexing.retry(2).await,
        Ok(RetryOutcome::Embedded(EmbedOutcome::Indexed { chunks: 1 }))
    );
    assert_eq!(documents.row(2).status, DocumentStatus::Indexed);
    assert_eq!(indexing.retry(3).await, Ok(RetryOutcome::NothingToDo));
    assert!(indexing.retry(99).await.is_err(), "unknown document");
}

#[tokio::test]
async fn reindexing_needs_a_model() {
    let (indexing, documents) = indexing(FixedEmbeddingSource::none()).await;
    assert!(matches!(
        indexing.reindex_all().await,
        Err(IndexingError::Unavailable(_))
    ));
    assert_eq!(
        documents.row(3).status,
        DocumentStatus::Indexed,
        "nothing marked pending"
    );
    assert_eq!(indexing.report().await.unwrap().model_id, None);
}
