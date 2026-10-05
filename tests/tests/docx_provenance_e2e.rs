//! A DOCX keeps its structure through the real pipeline: parse → chunk → SQLite → embeddings →
//! retrieval → RAG. Every chunk knows its document, its section path and (for a table) which
//! table; nothing in it refers to a page; and it travels through the same embedding and
//! retrieval code as every other format.

mod support;

use std::sync::Arc;

use nlmx_application::{
    ports::{CancelFlag, DocumentRepository},
    services::{
        rag::{AnswerStatus, RagEngine, RagOptions},
        retrieval::HybridRetriever,
        retriever::{Retriever, RetrieverOptions},
    },
};
use nlmx_domain::{
    document_type::DocumentType, ingestion::DocumentId, parsed::DocumentChunk,
    retrieval::RetrievalFilter, source::SourceLocation,
};
use nlmx_testing::FakeLlmProvider;
use support::multiformat::{App, imported, office_fixture};

struct Manual {
    app: App,
    id: DocumentId,
    chunks: Vec<DocumentChunk>,
}

impl Manual {
    async fn new(name: &str) -> Self {
        let app = App::new(name, true);
        let path = app.user_file(&office_fixture("manual.docx"), "Manual.docx");
        let id = imported(app.ingestion.import(&path).await).0;
        let chunks = app.db.chunks_of(id).await.unwrap();
        Self { app, id, chunks }
    }

    fn retriever(&self) -> Arc<Retriever> {
        Arc::new(Retriever::new(Arc::new(HybridRetriever::new(
            self.app.model.clone(),
            self.app.db.clone(),
            self.app.db.clone(),
            self.app.db.clone(),
        ))))
    }

    fn in_section(&self, path: &[&str]) -> Vec<&DocumentChunk> {
        self.chunks
            .iter()
            .filter(|c| c.section_path == path)
            .collect()
    }
}

fn heading_path(chunk: &DocumentChunk) -> Vec<String> {
    match &chunk.location {
        SourceLocation::Docx { heading_path, .. } => heading_path.clone(),
        other => panic!("not a DOCX location: {other:?}"),
    }
}

#[tokio::test]
async fn every_chunk_carries_its_document_section_and_no_page() {
    let manual = Manual::new("docx-chunks").await;
    assert!(manual.chunks.len() > 5, "{} chunks", manual.chunks.len());
    for chunk in &manual.chunks {
        assert_eq!(chunk.document_id, manual.id);
        // The location's heading path is the section path of the chunk.
        assert_eq!(heading_path(chunk), chunk.section_path, "{:?}", chunk.text);
        assert!(!chunk.section_path.is_empty());
        assert_eq!(chunk.location.page_range(), None, "a DOCX has no pages");
        assert!(!chunk.location.previewable());
        assert!(chunk.location.boxes().is_empty());
        // The metadata is the document's.
        let m = &chunk.metadata;
        assert_eq!(m.document_type, DocumentType::Docx);
        assert_eq!(m.file_name.as_deref(), Some("Manual.docx"));
        assert_eq!(m.document_title.as_deref(), Some("Contrato de Serviços"));
        assert_eq!(m.language.as_deref(), Some("pt-BR"));
    }
    // The tree of the example: Arquitetura › Backend is a section of its own.
    let backend = manual.in_section(&["Arquitetura", "Backend"]);
    assert!(backend.len() >= 2, "a long section makes several chunks");
    for chunk in &backend {
        assert_eq!(chunk.location.label(), "Arquitetura › Backend");
    }
    let sections: Vec<_> = manual
        .chunks
        .iter()
        .map(|c| c.section_path.join(" › "))
        .collect();
    for expected in [
        "Introdução",
        "Arquitetura",
        "Arquitetura › Backend",
        "Arquitetura › Frontend",
        "Deploy",
    ] {
        assert!(
            sections.iter().any(|s| s == expected),
            "{expected}: {sections:?}"
        );
    }
    // A section's chunks are contiguous and in reading order.
    let positions: Vec<usize> = sections
        .iter()
        .enumerate()
        .filter(|(_, s)| *s == "Arquitetura › Backend")
        .map(|(i, _)| i)
        .collect();
    assert!(positions.windows(2).all(|w| w[1] == w[0] + 1));
}

#[tokio::test]
async fn lists_stay_in_their_section_and_tables_say_which_table_they_are() {
    let manual = Manual::new("docx-lists-tables").await;
    let list = manual
        .chunks
        .iter()
        .find(|c| c.text.contains("Autenticação por token"))
        .expect("the list is indexed");
    assert_eq!(list.section_path, ["Arquitetura", "Backend"]);

    let first = manual
        .chunks
        .iter()
        .find(|c| c.text.contains("Tela: Login"))
        .expect("the first table is indexed");
    assert!(
        first
            .text
            .contains("Tabela 1 — Telas principais: Tela | Responsável | Estado")
    );
    assert!(first.text.contains("Responsável: Marina"));
    assert_eq!(first.section_path, ["Arquitetura", "Frontend"]);
    assert!(matches!(
        first.location,
        SourceLocation::Docx { table: Some(1), .. }
    ));
    assert_eq!(first.location.label(), "Arquitetura › Frontend, tabela 1");

    let second = manual
        .chunks
        .iter()
        .find(|c| c.text.contains("Ambiente: Produção"))
        .expect("the second table is indexed");
    assert_eq!(second.section_path, ["Deploy"]);
    assert!(matches!(
        second.location,
        SourceLocation::Docx { table: Some(2), .. }
    ));
}

#[tokio::test]
async fn the_location_is_stored_as_a_docx_location_without_page_fields() {
    let manual = Manual::new("docx-sql").await;
    let sql = manual.app.sql();
    let stored: i64 = sql
        .query_row(
            "SELECT COUNT(*) FROM chunk_provenance WHERE json_extract(locator, '$.kind') = 'docx'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored as usize, manual.chunks.len());
    let with_page: i64 = sql
        .query_row(
            "SELECT COUNT(*) FROM chunk_provenance WHERE locator LIKE '%page%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(with_page, 0);
    let format: String = sql
        .query_row("SELECT format FROM documents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(format, "docx");
    let previewable: i64 = sql
        .query_row("SELECT previewable FROM documents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(previewable, 0);
}

#[tokio::test]
async fn embeddings_are_the_same_as_for_any_other_format() {
    let manual = Manual::new("docx-embeddings").await;
    let embedded: i64 = manual.app.count("SELECT COUNT(*) FROM chunk_embeddings");
    assert_eq!(
        embedded as usize,
        manual.chunks.len(),
        "one vector per chunk"
    );
}

#[tokio::test]
async fn retrieval_and_the_rag_cite_the_section_and_never_a_page() {
    let manual = Manual::new("docx-rag").await;
    let ctx = manual
        .retriever()
        .retrieve(
            "Servicos descreve o comportamento esperado do componente",
            &RetrieverOptions {
                top_k: 10,
                max_per_document: 5,
                min_score: 0.0,
                filter: RetrievalFilter {
                    documents: Some(vec![manual.id]),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let passage = ctx
        .passages
        .iter()
        .find(|p| p.content.contains("Servicos"))
        .expect("the Backend text is retrieved");
    let source = &passage.provenance;
    assert_eq!(source.document_type(), DocumentType::Docx);
    assert_eq!(source.document_name, "Manual.docx");
    assert_eq!(source.label(), "Manual.docx · Arquitetura › Backend");
    assert_eq!(passage.source.label, source.label());
    assert_eq!(passage.metadata.section_path, ["Arquitetura", "Backend"]);
    assert_eq!(source.location().page_range(), None);
    assert!(!source.previewable());

    // The RAG gets the same kind of source as for any other format.
    let llm = Arc::new(FakeLlmProvider::available().answering("Veja [1][2][3][4][5]."));
    let answer = RagEngine::new(manual.retriever(), manual.app.db.clone(), llm.clone())
        .ask(
            "O que descreve o componente Servicos?",
            &RagOptions {
                retriever: RetrieverOptions {
                    top_k: 10,
                    max_per_document: 5,
                    min_score: 0.0,
                    ..Default::default()
                },
                min_relevance: 0.0,
                ..Default::default()
            },
            &|_| {},
            CancelFlag::default(),
        )
        .await
        .unwrap();
    assert_eq!(answer.status, AnswerStatus::Answered);
    assert!(!answer.citations.is_empty());
    for citation in &answer.citations {
        assert_eq!(citation.provenance.document_type(), DocumentType::Docx);
        assert!(citation.viewer_target().is_none());
        assert!(!citation.provenance.label().contains("p."));
    }
    let prompt = &llm.requests()[0].user;
    assert!(
        prompt.contains(r#"tipo="docx" localizacao="Arquitetura › Backend""#),
        "{prompt}"
    );
    assert!(
        !prompt.contains("paginas="),
        "no page in the prompt: {prompt}"
    );
}
