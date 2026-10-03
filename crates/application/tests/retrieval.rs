//! `HybridRetriever` with in-memory fakes: modes, weights, filters and fallbacks.

use std::sync::Arc;

use nlmx_application::{
    ports::{EmbeddingProvider, VectorStore},
    services::retrieval::{HybridRetriever, RetrievalError, RetrievalMode},
};
use nlmx_domain::{
    embedding::EmbeddingPurpose,
    retrieval::{PageRange, RetrievalFilter, RetrievalOptions},
    vectors::EmbeddingSpace,
};
use nlmx_testing::{FakeCorpus, FakeEmbeddingProvider, FakeVectorStore, FixedEmbeddingSource};

const TEXTS: &[(i64, i64, u32, &str)] = &[
    (1, 1, 1, "carência de 180 dias para internação"),
    (2, 1, 2, "cobertura de consultas e exames"),
    (3, 2, 1, "prazo de carência para exames"),
    (4, 2, 5, "rescisão do contrato com aviso prévio"),
];

async fn setup(with_embeddings: bool) -> HybridRetriever {
    let mut corpus = FakeCorpus::default().collection(10, vec![2]);
    let mut store = FakeVectorStore::default();
    for &(chunk, doc, page, text) in TEXTS {
        corpus = corpus.chunk(chunk, doc, page, text);
        store = store.with_chunk(chunk, doc);
    }
    let embeddings = FakeEmbeddingProvider::default();
    let index = store
        .create_index(&EmbeddingSpace::from_identity(
            &embeddings.identity().await.unwrap(),
        ))
        .await
        .unwrap();
    for &(chunk, _, _, text) in TEXTS {
        let v = embeddings
            .embed(text, EmbeddingPurpose::Passage)
            .await
            .unwrap();
        store.insert_embedding(index.id, chunk, &v).await.unwrap();
    }
    let corpus = Arc::new(corpus);
    let provider: Option<Arc<dyn EmbeddingProvider>> =
        with_embeddings.then(|| Arc::new(embeddings) as Arc<dyn EmbeddingProvider>);
    HybridRetriever::new(
        Arc::new(FixedEmbeddingSource(provider)),
        Arc::new(store),
        corpus.clone(),
        corpus,
    )
}

fn options(top_k: usize, semantic: f32, lexical: f32) -> RetrievalOptions {
    RetrievalOptions {
        top_k,
        semantic_weight: semantic,
        lexical_weight: lexical,
        filter: RetrievalFilter::default(),
    }
}

fn ids(result: &nlmx_application::services::retrieval::RetrievalResult) -> Vec<i64> {
    result.chunks.iter().map(|c| c.chunk.chunk_id).collect()
}

#[tokio::test]
async fn hybrid_results_are_hydrated_and_explained() {
    let r = setup(true).await;
    let result = r
        .retrieve("  prazo de carência  ", &options(3, 0.6, 0.4))
        .await
        .unwrap();
    assert_eq!(result.mode, RetrievalMode::Hybrid);
    assert!(result.warnings.is_empty());
    assert_eq!(ids(&result)[0], 3, "matches both mechanisms best");
    let top = &result.chunks[0];
    assert_eq!(top.chunk.text, "prazo de carência para exames");
    assert!(top.semantic.is_some() && top.lexical.is_some());
    assert!(result.chunks.iter().all(|c| (0.0..=1.0).contains(&c.score)));
    assert!(result.chunks.windows(2).all(|w| w[0].score >= w[1].score));
    assert!(result.chunks.len() <= 3);
}

#[tokio::test]
async fn weights_switch_mechanisms_off() {
    let r = setup(true).await;
    let semantic = r
        .retrieve("carência exames", &options(4, 1.0, 0.0))
        .await
        .unwrap();
    assert_eq!(semantic.mode, RetrievalMode::SemanticOnly);
    assert!(semantic.chunks.iter().all(|c| c.lexical.is_none()));
    // Semantic-only order is the order of semantic ranks.
    let ranks: Vec<usize> = semantic
        .chunks
        .iter()
        .map(|c| c.semantic.unwrap().rank)
        .collect();
    assert_eq!(ranks, (1..=ranks.len()).collect::<Vec<_>>());

    let lexical = r
        .retrieve("carência exames", &options(4, 0.0, 1.0))
        .await
        .unwrap();
    assert_eq!(lexical.mode, RetrievalMode::LexicalOnly);
    assert!(lexical.chunks.iter().all(|c| c.semantic.is_none()));
    assert_eq!(
        ids(&lexical),
        [3, 1, 2],
        "only chunks containing the terms, by lexical rank"
    );
}

#[tokio::test]
async fn without_an_embedding_model_retrieval_is_lexical_with_a_warning() {
    let r = setup(false).await;
    let result = r.retrieve("rescisão", &options(3, 0.6, 0.4)).await.unwrap();
    assert_eq!(result.mode, RetrievalMode::LexicalOnly);
    assert_eq!(ids(&result), [4]);
    assert!(result.warnings[0].contains("embeddings"));
    // Semantic only and no model: nothing can run.
    assert!(matches!(
        r.retrieve("rescisão", &options(3, 1.0, 0.0)).await,
        Err(RetrievalError::Unavailable(_))
    ));
}

#[tokio::test]
async fn filters_apply_to_both_mechanisms() {
    let r = setup(true).await;
    let with = |filter: RetrievalFilter| RetrievalOptions {
        filter,
        ..options(10, 0.5, 0.5)
    };

    let doc1 = r
        .retrieve(
            "carência exames",
            &with(RetrievalFilter {
                documents: Some(vec![1]),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    assert!(!doc1.chunks.is_empty() && doc1.chunks.iter().all(|c| c.chunk.document_id == 1));

    let collection = r
        .retrieve(
            "carência exames",
            &with(RetrievalFilter {
                collections: Some(vec![10]),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    assert!(
        !collection.chunks.is_empty() && collection.chunks.iter().all(|c| c.chunk.document_id == 2)
    );

    let page = r
        .retrieve(
            "contrato carência",
            &with(RetrievalFilter {
                pages: Some(PageRange { from: 5, to: 9 }),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    assert_eq!(ids(&page), [4]);
}

#[tokio::test]
async fn degenerate_queries_and_options() {
    let r = setup(true).await;
    assert!(
        r.retrieve("   ", &options(3, 0.5, 0.5))
            .await
            .unwrap()
            .chunks
            .is_empty()
    );
    // Only stopwords: no lexical terms, semantic still runs.
    let stop = r.retrieve("o que é?", &options(3, 0.5, 0.5)).await.unwrap();
    assert_eq!(stop.mode, RetrievalMode::SemanticOnly);
    assert!(matches!(
        r.retrieve("x", &options(0, 0.5, 0.5)).await,
        Err(RetrievalError::InvalidOptions(_))
    ));
    assert!(matches!(
        r.retrieve("x", &options(3, 0.0, 0.0)).await,
        Err(RetrievalError::InvalidOptions(_))
    ));
}
