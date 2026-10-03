//! Semantic vs lexical vs hybrid retrieval over real SQLite (FTS5 + sqlite-vec).
//!
//! `concept_embedder_*` runs always, with a deterministic embedder that maps Portuguese/English
//! synonyms to the same "concept" dimensions (unknown words contribute nothing): it isolates what
//! each mechanism is good at. `real_model_*` repeats the comparison with the real Qwen3 model
//! (`make test-llama`).

use std::{path::Path, sync::Arc};

use nlmx_application::{
    ports::{BoxFuture, EmbeddingProvider, VectorStore},
    services::retrieval::{HybridRetriever, RetrievalMode, RetrievalResult},
};
use nlmx_domain::{
    embedding::{EmbeddingError, EmbeddingPurpose, ModelIdentity},
    retrieval::{RetrievalFilter, RetrievalOptions},
    vectors::EmbeddingSpace,
};
use nlmx_store_sqlite::Database;

const CORPUS: &[(i64, &str)] = &[
    (1, "A carência para internação hospitalar é de 180 dias."),
    (2, "A cobertura inclui consultas e exames laboratoriais."),
    (
        3,
        "O contrato pode ser rescindido com aviso prévio de 30 dias.",
    ),
    (
        4,
        "Conforme a Resolução Normativa 465 da ANS, o rol de cobertura é atualizado.",
    ),
    (5, "Asma (CID J45) tem tratamento ambulatorial."),
];

/// Concept dimensions: 0 waiting period, 1 coverage, 2 termination, 3 care/hospital; 4 = small bias
/// (so a text without known words still has a direction, but a weak one).
struct ConceptEmbedder;

fn concept(word: &str) -> Option<usize> {
    match word {
        "carência" | "carencia" | "waiting" | "period" | "prazo" => Some(0),
        "cobertura" | "coverage" | "covered" | "exames" | "consultas" | "tests" => Some(1),
        "rescindido" | "rescisão" | "cancel" | "cancelar" | "terminate" | "contrato"
        | "contract" => Some(2),
        "internação" | "hospitalar" | "hospitalization" | "hospital" | "tratamento"
        | "ambulatorial" | "treatment" => Some(3),
        _ => None,
    }
}

impl ConceptEmbedder {
    fn vector(text: &str) -> Vec<f32> {
        let mut v = [0.0f32; 5];
        v[4] = 0.05;
        for word in text.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
            if let Some(i) = concept(word) {
                v[i] += 1.0;
            }
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / norm).collect()
    }
}

impl EmbeddingProvider for ConceptEmbedder {
    fn model_id(&self) -> &str {
        "concepts"
    }
    fn identity(&self) -> BoxFuture<'_, Result<ModelIdentity, EmbeddingError>> {
        Box::pin(async {
            Ok(ModelIdentity {
                id: "concepts".into(),
                file_name: "c".into(),
                file_size: 0,
                dimensions: 5,
                context_length: 64,
                parameters: 0,
            })
        })
    }
    fn dimensions(&self) -> BoxFuture<'_, Result<u32, EmbeddingError>> {
        Box::pin(async { Ok(5) })
    }
    fn embed<'a>(
        &'a self,
        text: &'a str,
        _: EmbeddingPurpose,
    ) -> BoxFuture<'a, Result<Vec<f32>, EmbeddingError>> {
        Box::pin(async move { Ok(Self::vector(text)) })
    }
    fn embed_batch<'a>(
        &'a self,
        texts: &'a [String],
        _: EmbeddingPurpose,
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, EmbeddingError>> {
        Box::pin(async move { Ok(texts.iter().map(|t| Self::vector(t)).collect()) })
    }
}

/// A database with the corpus (one document) and its vectors from `embedder`.
async fn retriever(name: &str, embedder: Arc<dyn EmbeddingProvider>) -> HybridRetriever {
    let dir = std::env::temp_dir().join(format!("nlmx-hybrid-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("db.sqlite3");
    let db = Database::open(&path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO documents (id, sha256, title, original_filename, library_path, file_size) VALUES (1, ?1, 'Contrato', 'c.pdf', 'c', 1)",
        ["f".repeat(64)],
    )
    .unwrap();
    for &(id, text) in CORPUS {
        conn.execute(
            "INSERT INTO document_chunks (id, document_id, ordinal, text, token_count, page_start, page_end, content_hash)
             VALUES (?1, 1, ?1, ?2, 10, 1, 1, ?3)",
            rusqlite::params![id, text, format!("h{id}")],
        )
        .unwrap();
    }
    let space = EmbeddingSpace::from_identity(&embedder.identity().await.unwrap());
    let index = db.create_index(&space).await.unwrap();
    let texts: Vec<String> = CORPUS.iter().map(|(_, t)| t.to_string()).collect();
    let vectors = embedder
        .embed_batch(&texts, EmbeddingPurpose::Passage)
        .await
        .unwrap();
    let items: Vec<(i64, Vec<f32>)> = CORPUS.iter().map(|(id, _)| *id).zip(vectors).collect();
    db.insert_batch(index.id, &items).await.unwrap();
    let db = Arc::new(db);
    HybridRetriever::new(
        Arc::new(nlmx_testing::FixedEmbeddingSource(Some(embedder))),
        db.clone(),
        db.clone(),
        db,
    )
}

struct Comparison {
    semantic: RetrievalResult,
    lexical: RetrievalResult,
    hybrid: RetrievalResult,
}

async fn compare(r: &HybridRetriever, query: &str) -> Comparison {
    let opts = |s: f32, l: f32| RetrievalOptions {
        top_k: 3,
        semantic_weight: s,
        lexical_weight: l,
        filter: RetrievalFilter::default(),
    };
    let (semantic, lexical, hybrid) = (opts(1.0, 0.0), opts(0.0, 1.0), opts(0.6, 0.4));
    let c = Comparison {
        semantic: r.retrieve(query, &semantic).await.unwrap(),
        lexical: r.retrieve(query, &lexical).await.unwrap(),
        hybrid: r.retrieve(query, &hybrid).await.unwrap(),
    };
    let fmt = |res: &RetrievalResult| {
        res.chunks
            .iter()
            .map(|h| format!("#{} {:.2}", h.chunk.chunk_id, h.score))
            .collect::<Vec<_>>()
            .join(", ")
    };
    eprintln!(
        "\n{query}\n  semântico: [{}]\n  lexical:   [{}]\n  híbrido:   [{}]",
        fmt(&c.semantic),
        fmt(&c.lexical),
        fmt(&c.hybrid)
    );
    c
}

fn top(result: &RetrievalResult) -> Option<i64> {
    result.chunks.first().map(|c| c.chunk.chunk_id)
}

#[tokio::test]
async fn concept_embedder_semantic_and_lexical_complement_each_other() {
    let r = retriever("concepts", Arc::new(ConceptEmbedder)).await;

    // 1. Translation with no shared words: only semantic finds it.
    let c = compare(&r, "What is the waiting period for hospitalization?").await;
    assert_eq!(top(&c.semantic), Some(1));
    assert!(
        c.lexical.chunks.is_empty(),
        "no Portuguese word in the query"
    );
    assert_eq!(top(&c.hybrid), Some(1));
    assert_eq!(c.hybrid.mode, RetrievalMode::Hybrid);

    // 2. Exact identifier: only lexical is confident.
    let c = compare(&r, "Resolução Normativa 465").await;
    assert_eq!(top(&c.lexical), Some(4));
    assert!(
        c.semantic.chunks.iter().all(|h| h.score < 0.2),
        "no semantic signal for a code"
    );
    assert_eq!(top(&c.hybrid), Some(4));

    let c = compare(&r, "CID J45").await;
    assert_eq!(top(&c.lexical), Some(5));
    assert_eq!(top(&c.hybrid), Some(5));

    // 3. Paraphrase in English: semantic finds the termination clause, lexical has nothing.
    let c = compare(&r, "How do I cancel the contract?").await;
    assert_eq!(top(&c.semantic), Some(3));
    assert!(c.lexical.chunks.is_empty());
    assert_eq!(top(&c.hybrid), Some(3));

    // 4. Same words and meaning: both agree, and hybrid ranks it with the highest score.
    let c = compare(&r, "carência para internação").await;
    assert_eq!(
        (top(&c.semantic), top(&c.lexical), top(&c.hybrid)),
        (Some(1), Some(1), Some(1))
    );
    let best = &c.hybrid.chunks[0];
    assert!(best.semantic.is_some() && best.lexical.is_some());
    assert!(best.score > c.hybrid.chunks[1].score);
}

#[tokio::test]
#[ignore = "needs runtime/llama and models/Qwen3-Embedding-0.6B-Q8_0.gguf"]
async fn real_model_hybrid_gets_every_query_right() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let config =
        nlmx_embed_llama::EmbeddingConfig::load(&root.join("models/embedding.example.json"))
            .unwrap();
    let dir = std::env::temp_dir().join(format!("nlmx-hybrid-real-run-{}", std::process::id()));
    let provider = Arc::new(nlmx_embed_llama::provider(
        config,
        nlmx_embed_llama::llama_server_path().unwrap(),
        &dir,
    ));
    let r = retriever("real", provider.clone()).await;

    for (query, expected) in [
        ("What is the waiting period for hospitalization?", 1),
        ("Resolução Normativa 465", 4),
        ("CID J45", 5),
        ("How do I cancel the contract?", 3),
        ("Quais exames estão cobertos?", 2),
    ] {
        let c = compare(&r, query).await;
        assert_eq!(top(&c.hybrid), Some(expected), "{query}");
    }
    provider.shutdown().await;
}
