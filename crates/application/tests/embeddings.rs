//! `EmbedDocuments`: indexing chunks with the current model, and keeping documents pending otherwise.

use std::sync::Arc;

use nlmx_application::{
    ports::{BoxFuture, DocumentRepository, EmbeddingProvider, NewDocument, VectorStore},
    use_cases::{EmbedDocuments, EmbedOutcome},
};
use nlmx_domain::{
    embedding::{EmbeddingError, EmbeddingPurpose, ModelIdentity},
    ingestion::DocumentStatus,
    vectors::{EmbeddingSpace, VectorFilter},
};
use nlmx_testing::{
    FakeCorpus, FakeDocumentRepository, FakeEmbeddingProvider, FakeVectorStore,
    FixedEmbeddingSource,
};

struct Setup {
    embed: EmbedDocuments,
    documents: Arc<FakeDocumentRepository>,
    vectors: Arc<FakeVectorStore>,
}

async fn setup(source: Arc<FixedEmbeddingSource>) -> Setup {
    let documents = Arc::new(FakeDocumentRepository::default());
    for sha in ['a', 'b'] {
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
    documents.force_status(1, DocumentStatus::Embedding);
    documents.force_status(2, DocumentStatus::Embedding);
    let corpus = FakeCorpus::default()
        .chunk(1, 1, 1, "carência de 180 dias")
        .chunk(2, 1, 2, "cobertura de exames")
        .chunk(3, 2, 1, "rescisão do contrato");
    let vectors = Arc::new(
        FakeVectorStore::default()
            .with_chunk(1, 1)
            .with_chunk(2, 1)
            .with_chunk(3, 2),
    );
    let embed = EmbedDocuments {
        progress: None,
        embeddings: source,
        vectors: vectors.clone(),
        chunks: Arc::new(corpus),
        documents: documents.clone(),
        batch_size: 1,
    };
    Setup {
        embed,
        documents,
        vectors,
    }
}

async fn index_id(vectors: &FakeVectorStore, dims: u32) -> i64 {
    let identity = FakeEmbeddingProvider { dimensions: dims }
        .identity()
        .await
        .unwrap();
    vectors
        .create_index(&EmbeddingSpace::from_identity(&identity))
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn indexes_every_chunk_with_the_models_dimension_and_marks_the_document() {
    let s = setup(FixedEmbeddingSource::of(FakeEmbeddingProvider {
        dimensions: 12,
    }))
    .await;
    assert_eq!(
        s.embed.embed_document(1).await,
        EmbedOutcome::Indexed { chunks: 2 }
    );
    assert_eq!(s.documents.row(1).status, DocumentStatus::Indexed);

    let index = index_id(&s.vectors, 12).await;
    assert_eq!(s.vectors.count(index).await.unwrap(), 2);
    let query = FakeEmbeddingProvider { dimensions: 12 }
        .embed("carência", EmbeddingPurpose::Query)
        .await
        .unwrap();
    let hits = s
        .vectors
        .search(index, &query, 1, &VectorFilter::default())
        .await
        .unwrap();
    assert_eq!(hits[0].chunk_id, 1);

    // Running it again replaces the vectors instead of duplicating them.
    s.embed.embed_document(1).await;
    assert_eq!(s.vectors.count(index).await.unwrap(), 2);
}

#[tokio::test]
async fn without_a_model_documents_keep_waiting() {
    let s = setup(FixedEmbeddingSource::none()).await;
    assert_eq!(s.embed.embed_document(1).await, EmbedOutcome::NoModel);
    assert_eq!(s.documents.row(1).status, DocumentStatus::Embedding);
    assert!(s.embed.embed_pending().await.is_empty());
}

struct BrokenProvider;

impl EmbeddingProvider for BrokenProvider {
    fn model_id(&self) -> &str {
        "broken"
    }
    fn identity(&self) -> BoxFuture<'_, Result<ModelIdentity, EmbeddingError>> {
        Box::pin(async {
            Ok(ModelIdentity {
                id: "broken".into(),
                file_name: "b".into(),
                file_size: 0,
                dimensions: 4,
                context_length: 8,
                parameters: 0,
            })
        })
    }
    fn dimensions(&self) -> BoxFuture<'_, Result<u32, EmbeddingError>> {
        Box::pin(async { Ok(4) })
    }
    fn embed<'a>(
        &'a self,
        _: &'a str,
        _: EmbeddingPurpose,
    ) -> BoxFuture<'a, Result<Vec<f32>, EmbeddingError>> {
        Box::pin(async { Err(EmbeddingError::Unavailable("servidor caiu".into())) })
    }
    fn embed_batch<'a>(
        &'a self,
        _: &'a [String],
        _: EmbeddingPurpose,
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, EmbeddingError>> {
        Box::pin(async { Err(EmbeddingError::Unavailable("servidor caiu".into())) })
    }
}

#[tokio::test]
async fn model_errors_keep_the_document_pending_with_the_reason() {
    let s = setup(FixedEmbeddingSource::of(BrokenProvider)).await;
    let outcome = s.embed.embed_document(1).await;
    assert!(
        matches!(outcome, EmbedOutcome::Failed(ref m) if m.contains("servidor caiu")),
        "{outcome:?}"
    );
    let row = s.documents.row(1);
    assert_eq!(
        row.status,
        DocumentStatus::Embedding,
        "not failed: it can be retried"
    );
    assert!(row.error.unwrap().contains("servidor caiu"));
}

#[tokio::test]
async fn pending_documents_are_embedded_and_reindex_starts_over() {
    let s = setup(FixedEmbeddingSource::of(FakeEmbeddingProvider {
        dimensions: 8,
    }))
    .await;
    let outcomes = s.embed.embed_pending().await;
    assert_eq!(
        outcomes,
        [
            (1, EmbedOutcome::Indexed { chunks: 2 }),
            (2, EmbedOutcome::Indexed { chunks: 1 })
        ]
    );
    assert!(s.embed.embed_pending().await.is_empty(), "nothing left");

    assert_eq!(
        s.embed.reindex_all().await.len(),
        2,
        "indexed documents are embedded again"
    );
    assert_eq!(s.documents.row(2).status, DocumentStatus::Indexed);
}
