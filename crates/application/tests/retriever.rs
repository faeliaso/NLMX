//! `Retriever`: duplicates, neighbour joining, limits, Top-K, grouping and the passage shape.

use std::sync::Arc;

use nlmx_application::services::{
    retrieval::{HybridRetriever, RetrievalError},
    retriever::{Retriever, RetrieverOptions},
};
use nlmx_testing::{FakeCorpus, FakeVectorStore, FixedEmbeddingSource};

/// Lexical-only retriever over `chunks` (chunk id = ordinal; ids that differ by 1 are neighbours).
fn retriever(chunks: &[(i64, i64, u32, &str)]) -> Retriever {
    let corpus = chunks
        .iter()
        .fold(FakeCorpus::default(), |c, &(id, doc, page, text)| {
            c.chunk(id, doc, page, text)
        });
    let corpus = Arc::new(corpus);
    let hybrid = HybridRetriever::new(
        FixedEmbeddingSource::none(),
        Arc::new(FakeVectorStore::default()),
        corpus.clone(),
        corpus,
    );
    Retriever::new(Arc::new(hybrid))
}

fn options() -> RetrieverOptions {
    RetrieverOptions {
        min_score: 0.0,
        ..Default::default()
    }
}

#[tokio::test]
async fn neighbouring_chunks_become_one_passage_without_the_overlap() {
    let r = retriever(&[
        (
            1,
            1,
            2,
            "O prazo de carência começa na assinatura. Ponte entre trechos.",
        ),
        (
            2,
            1,
            3,
            "Ponte entre trechos. A carência termina em 180 dias.",
        ),
        (5, 1, 7, "Outra seção sobre carência, longe das demais."),
    ]);
    let ctx = r.retrieve("carência", &options()).await.unwrap();
    let joined = ctx
        .passages
        .iter()
        .find(|p| p.metadata.chunk_ids == [1, 2])
        .expect("1 and 2 joined");
    assert_eq!(
        joined.content,
        "O prazo de carência começa na assinatura. Ponte entre trechos.\n\nA carência termina em 180 dias.",
        "overlap not repeated"
    );
    assert_eq!(
        (
            joined.page,
            joined.source.page_start,
            joined.source.page_end
        ),
        (2, 2, 3)
    );
    assert_eq!(joined.source.label, "documento-1.pdf · pp. 2–3");
    assert_eq!(ctx.passages.len(), 2, "chunk 5 is not a neighbour");
    assert!(joined.metadata.matched_by.lexical && !joined.metadata.matched_by.semantic);

    let separate = r
        .retrieve(
            "carência",
            &RetrieverOptions {
                merge_adjacent: false,
                ..options()
            },
        )
        .await
        .unwrap();
    assert_eq!(separate.passages.len(), 3);
}

#[tokio::test]
async fn neighbours_from_different_sections_are_not_joined() {
    let corpus = FakeCorpus::default()
        .chunk(1, 1, 1, "Carência para consultas.")
        .in_section("1. Consultas")
        .chunk(2, 1, 1, "Carência para internação.")
        .in_section("2. Internação");
    let corpus = Arc::new(corpus);
    let hybrid = HybridRetriever::new(
        FixedEmbeddingSource::none(),
        Arc::new(FakeVectorStore::default()),
        corpus.clone(),
        corpus,
    );
    let ctx = Retriever::new(Arc::new(hybrid))
        .retrieve("carência", &options())
        .await
        .unwrap();
    assert_eq!(ctx.passages.len(), 2);
    assert!(ctx.passages.iter().all(|p| p.metadata.chunk_ids.len() == 1));
    assert!(
        ctx.passages
            .iter()
            .any(|p| p.source.label == "documento-1.pdf · p. 1"
                && p.source.section.as_deref() == Some("2. Internação"))
    );
}

#[tokio::test]
async fn the_same_content_in_two_documents_is_returned_once() {
    let r = retriever(&[
        (1, 1, 1, "A cobertura inclui exames laboratoriais."),
        (10, 2, 4, "A cobertura inclui exames laboratoriais."),
        (20, 3, 1, "Exames de imagem não têm cobertura."),
    ]);
    let ctx = r.retrieve("cobertura exames", &options()).await.unwrap();
    assert_eq!(ctx.passages.len(), 2);
    let kept = ctx
        .passages
        .iter()
        .find(|p| p.content.starts_with("A cobertura inclui"))
        .unwrap();
    // Whichever copy is kept, the other one is recorded as a duplicate (document, chunk).
    let other = if kept.chunk_id == 1 { (2, 10) } else { (1, 1) };
    assert_eq!(kept.metadata.duplicates, [other]);
}

#[tokio::test]
async fn near_duplicates_are_dropped() {
    let r = retriever(&[
        (
            1,
            1,
            1,
            "O reembolso de consultas é feito em até trinta dias após o pedido.",
        ),
        (
            10,
            2,
            1,
            "O reembolso de consultas é feito em até trinta dias após o pedido formal.",
        ),
        (
            20,
            3,
            1,
            "Consultas de urgência não exigem pedido de reembolso.",
        ),
    ]);
    let ctx = r
        .retrieve("reembolso consultas pedido", &options())
        .await
        .unwrap();
    let ids: Vec<i64> = ctx.passages.iter().map(|p| p.chunk_id).collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert!(ids.contains(&20));
    let survivor = ctx.passages.iter().find(|p| p.chunk_id != 20).unwrap();
    assert_eq!(survivor.metadata.duplicates.len(), 1);

    let strict = r
        .retrieve(
            "reembolso consultas pedido",
            &RetrieverOptions {
                near_duplicate_threshold: 1.0,
                ..options()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        strict.passages.len(),
        3,
        "threshold 1.0 keeps near-duplicates"
    );
}

#[tokio::test]
async fn limits_per_document_min_score_and_top_k() {
    // Non-adjacent chunks (even ids) with different texts, so nothing is joined or deduplicated.
    let texts: Vec<String> = (0..5)
        .map(|i| format!("Glosa do procedimento número {i} da tabela {}.", i * 7))
        .collect();
    let mut chunks: Vec<(i64, i64, u32, &str)> = texts
        .iter()
        .enumerate()
        .map(|(i, t)| (i as i64 * 2, 1, i as u32 + 1, t.as_str()))
        .collect();
    chunks.push((100, 2, 1, "Glosa em outro documento."));
    chunks.push((200, 3, 1, "Glosa no terceiro documento."));
    let r = retriever(&chunks);

    let ctx = r
        .retrieve(
            "glosa",
            &RetrieverOptions {
                max_per_document: 2,
                ..options()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        ctx.passages.iter().filter(|p| p.document_id == 1).count(),
        2
    );
    assert_eq!(ctx.passages.len(), 4);

    let top = r
        .retrieve(
            "glosa",
            &RetrieverOptions {
                top_k: 2,
                max_per_document: 10,
                ..options()
            },
        )
        .await
        .unwrap();
    assert_eq!(top.passages.len(), 2);

    let none = r
        .retrieve(
            "glosa",
            &RetrieverOptions {
                min_score: 1.0,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(none.passages.iter().all(|p| p.score >= 1.0));
    let empty = r.retrieve("inexistente", &options()).await.unwrap();
    assert!(empty.passages.is_empty() && empty.documents.is_empty());
}

#[tokio::test]
async fn passages_are_grouped_by_document_in_page_order() {
    let r = retriever(&[
        (1, 1, 5, "Franquia e coparticipação: franquia anual."),
        (3, 1, 2, "Franquia mensal."),
        (10, 2, 9, "Franquia franquia franquia franquia."),
    ]);
    let ctx = r.retrieve("franquia", &options()).await.unwrap();
    assert_eq!(
        ctx.passages[0].document_id, 2,
        "best passage first (most occurrences)"
    );
    let docs: Vec<i64> = ctx.documents.iter().map(|g| g.document_id).collect();
    assert_eq!(docs, [2, 1], "groups ordered by best score");
    let doc1 = &ctx.documents[1];
    let pages: Vec<u32> = doc1
        .passages
        .iter()
        .map(|&i| ctx.passages[i].page)
        .collect();
    assert_eq!(pages, [2, 5], "passages of a group in page order");
    assert_eq!(doc1.title, "Documento 1");
    assert!(
        ctx.documents
            .iter()
            .all(|g| ctx.passages[g.passages[0]].score <= g.best_score + f32::EPSILON)
    );
    // Each passage carries the fields the RAG needs.
    let p = &ctx.passages[0];
    assert!(!p.content.is_empty() && p.chunk_id == 10 && p.page == 9 && p.score > 0.0);
    assert_eq!(p.source.label, "documento-2.pdf · p. 9");
}

#[tokio::test]
async fn invalid_options_are_rejected() {
    let r = retriever(&[(1, 1, 1, "texto")]);
    for bad in [
        RetrieverOptions {
            top_k: 0,
            ..options()
        },
        RetrieverOptions {
            max_per_document: 0,
            ..options()
        },
        RetrieverOptions {
            min_score: 1.5,
            ..options()
        },
        RetrieverOptions {
            semantic_weight: 0.0,
            lexical_weight: 0.0,
            ..options()
        },
    ] {
        assert!(
            matches!(
                r.retrieve("texto", &bad).await,
                Err(RetrievalError::InvalidOptions(_))
            ),
            "{bad:?}"
        );
    }
}

#[tokio::test]
async fn templated_passages_with_different_numbers_are_not_near_duplicates() {
    // Same template, different values (trigram Jaccard ≈ 0.9 > 0.8): both are kept.
    let common = "cláusulas prazos e coberturas do contrato de saúde vigente para titulares e dependentes \
                  incluindo consultas exames internações cirurgias terapias e atendimentos de urgência \
                  conforme as regras gerais aprovadas pela operadora e pela agência reguladora nacional";
    let a = format!("Seção 58: {common}");
    let b = format!("Seção 137: {common}");
    assert!(nlmx_domain::retrieval::jaccard_trigrams(&a, &b) > 0.8);
    let r = retriever(&[(1, 1, 58, &a), (10, 2, 137, &b)]);
    let ctx = r
        .retrieve("cláusulas prazos coberturas", &options())
        .await
        .unwrap();
    assert_eq!(ctx.passages.len(), 2, "{:?}", ctx.passages);
}

/// An answer spread over several documents reaches the context from each of them, even when
/// one document has more than enough passages of its own to fill the Top-K.
#[tokio::test]
async fn every_document_that_matches_the_question_reaches_the_context() {
    // Document 1 has five strong passages; documents 2 and 3 have one weaker match each.
    let r = retriever(&[
        (10, 1, 1, "carência carência carência alfa"),
        (20, 1, 5, "carência carência carência bravo"),
        (30, 1, 9, "carência carência carência charlie"),
        (40, 1, 13, "carência carência carência delta"),
        (50, 1, 17, "carência carência carência echo"),
        (
            60,
            2,
            1,
            "a regra de carência do segundo documento, entre muitas outras palavras que diluem o termo",
        ),
        (
            70,
            3,
            1,
            "no terceiro documento a carência também é citada em uma frase bem longa e cheia de outras palavras",
        ),
        (
            80,
            4,
            1,
            "nada a ver com o assunto procurado, só frutas e legumes",
        ),
    ]);
    let top_four = |cover| RetrieverOptions {
        top_k: 4,
        max_per_document: 5,
        cover_documents: cover,
        ..options()
    };

    let without = r.retrieve("carência", &top_four(false)).await.unwrap();
    let documents = |ctx: &nlmx_application::services::retriever::RetrievalContext| {
        let mut d: Vec<i64> = ctx.passages.iter().map(|p| p.document_id).collect();
        d.sort();
        d.dedup();
        d
    };
    assert_eq!(
        documents(&without),
        [1],
        "the strongest document fills everything"
    );

    let with = r.retrieve("carência", &top_four(true)).await.unwrap();
    assert_eq!(with.passages.len(), 4, "the Top-K is still respected");
    assert_eq!(
        documents(&with),
        [1, 2, 3],
        "each matching document is represented"
    );
    // Never at the cost of a document's only passage, and not for a document that does not match.
    assert!(!documents(&with).contains(&4));
    assert!(
        with.passages.windows(2).all(|w| w[0].score >= w[1].score),
        "best first"
    );
    assert_eq!(with.passages[0].document_id, 1);
}
