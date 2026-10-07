//! "Adicionar nota" over the real adapters (SQLite + FTS5 + sqlite-vec, text parser,
//! normalizer, chunker, a deterministic embedder, the RAG with a scripted model): a note goes
//! through the same pipeline as a file, without a file, and is retrieved and cited like any
//! other source. PDFs and the other formats keep working next to it.

mod support;

use std::sync::Arc;

use nlmx_application::{
    ports::{CancelFlag, DocumentRepository, IndexingReader},
    services::{
        rag::{AnswerStatus, RagOptions},
        retriever::RetrieverOptions,
    },
    use_cases::{Enqueued, RemoveDocument},
};
use nlmx_domain::{
    document_type::DocumentType,
    ingestion::{DocumentStatus, ImportOutcome},
    source::SourceLocation,
};
use nlmx_testing::FakeLlmProvider;
use support::{Library, deterministic_embeddings, temp_dir};

const NOTE: &str = "Decisões do módulo de reembolso\r\n\r\nA carência do plano para internações é de 180 dias contados da adesão.   \r\n\r\n\r\n\r\nConsultas ambulatoriais não têm carência alguma.\r\n";

fn library(name: &str) -> Library {
    Library::new(temp_dir(name), deterministic_embeddings())
}

async fn add(lib: &Library, text: &str) -> (i64, ImportOutcome) {
    let enqueued = lib.ingestion.enqueue_note(text).await;
    let id = match &enqueued {
        Enqueued::New(id) | Enqueued::Retry(id) | Enqueued::Duplicate(id) => *id,
        other => panic!("{other:?}"),
    };
    (id, lib.ingestion.process(enqueued).await)
}

fn options() -> RagOptions {
    RagOptions {
        retriever: RetrieverOptions {
            min_score: 0.0,
            ..Default::default()
        },
        min_relevance: 0.1,
        ..Default::default()
    }
}

#[tokio::test]
async fn a_note_is_registered_as_a_source_without_a_file() {
    let lib = library("note-source");
    let (id, outcome) = add(&lib, NOTE).await;
    assert!(
        matches!(outcome, ImportOutcome::Imported { .. }),
        "{outcome:?}"
    );

    let record = lib.db.get(id).await.unwrap().unwrap();
    assert_eq!(record.document_type, DocumentType::Note);
    assert!(!record.previewable());
    assert_eq!(record.status, DocumentStatus::Indexed);
    assert!(record.library_path.is_empty());
    assert!(record.note_text.as_deref().unwrap().contains("180 dias"));
    // Nothing was written to the library: a note has no file.
    let library_dir = lib.dir.join("library");
    assert!(
        !library_dir.exists() || std::fs::read_dir(&library_dir).unwrap().next().is_none(),
        "a note must not create a file"
    );

    let summary = lib.ingestion.list().await.unwrap().remove(0);
    assert_eq!(summary.title, "Decisões do módulo de reembolso");
    assert_eq!(summary.document_type, DocumentType::Note);
    assert!(summary.chunk_count > 0 && summary.page_count.is_none());
    let details = lib.db.source_details(id).await.unwrap().unwrap();
    assert_eq!(details.title, "Decisões do módulo de reembolso");
    assert!(!details.document_type.previewable());
}

#[tokio::test]
async fn empty_and_whitespace_notes_are_refused_and_create_nothing() {
    let lib = library("note-empty");
    for text in ["", "   ", "\n\t \r\n"] {
        assert!(
            matches!(
                lib.ingestion.enqueue_note(text).await,
                Enqueued::Failed { id: None, .. }
            ),
            "{text:?}"
        );
    }
    assert!(lib.ingestion.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn the_same_note_twice_is_a_duplicate() {
    let lib = library("note-duplicate");
    let (first, _) = add(&lib, NOTE).await;
    assert!(matches!(
        lib.ingestion.enqueue_note(NOTE).await,
        Enqueued::Duplicate(id) if id == first
    ));
    // Only line ends and trailing spaces differ: it is the same note.
    assert!(matches!(
        lib.ingestion.enqueue_note(&NOTE.replace("\r\n", "\n")).await,
        Enqueued::Duplicate(id) if id == first
    ));
    assert_eq!(lib.ingestion.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_note_without_a_title_line_gets_one_from_its_content() {
    let lib = library("note-title");
    let (id, _) = add(
        &lib,
        "O prazo de carência para internações é de 180 dias contados da adesão ao plano.",
    )
    .await;
    let title = lib.db.source_details(id).await.unwrap().unwrap().title;
    assert!(
        title.starts_with("O prazo de carência") && title.ends_with('…'),
        "{title}"
    );
}

#[tokio::test]
async fn the_note_is_normalized_chunked_located_and_embedded() {
    let lib = library("note-pipeline");
    let long: String = (1..=60)
        .map(|n| format!("Parágrafo {n}: a carência de internação é de 180 dias e o reembolso de consultas leva 30 dias úteis, conforme o contrato."))
        .collect::<Vec<_>>()
        .join("\n\n");
    let (id, outcome) = add(&lib, &format!("Guia de prazos\n\n{long}")).await;
    assert!(
        matches!(outcome, ImportOutcome::Imported { .. }),
        "{outcome:?}"
    );

    let chunks = lib.db.chunks_of(id).await.unwrap();
    assert!(chunks.len() > 1, "a long note is split into chunks");
    let total = lib
        .db
        .get(id)
        .await
        .unwrap()
        .unwrap()
        .note_text
        .unwrap()
        .chars()
        .count() as u32;
    let mut previous_end = 0;
    for chunk in &chunks {
        assert!(!chunk.text.contains('\r'));
        assert_eq!(chunk.metadata.document_type, DocumentType::Note);
        let SourceLocation::Note { start, end } = chunk.location else {
            panic!(
                "a note chunk is located by characters: {:?}",
                chunk.location
            )
        };
        assert!(start < end && end <= total && start >= previous_end.min(start));
        previous_end = end;
        assert!(chunk.location.page_range().is_none() && !chunk.location.previewable());
    }

    // Every chunk has its vector in sqlite-vec (the embeddings ran as for any other source).
    let snapshot = IndexingReader::snapshot(&*lib.db, Some("fake-embedding".into()))
        .await
        .unwrap();
    assert_eq!(snapshot.model.map(|m| m.chunks), Some(chunks.len() as u32));
}

#[tokio::test]
async fn the_rag_retrieves_and_cites_a_note_like_any_other_source() {
    let mut lib = library("note-rag");
    lib.import(&["text.pdf"]).await; // a PDF next to it keeps working
    let (note, _) = add(&lib, NOTE).await;
    let llm = Arc::new(FakeLlmProvider::available().answering("A carência é de 180 dias [1]."));
    let answer = lib
        .rag(llm.clone())
        .ask(
            "Qual a carência do plano para internações?",
            &options(),
            &|_| {},
            CancelFlag::default(),
        )
        .await
        .unwrap();
    assert_eq!(answer.status, AnswerStatus::Answered, "{answer:?}");
    let source = answer
        .sources
        .iter()
        .find(|s| s.document_id == note)
        .expect("the note is among the passages");
    assert_eq!(source.provenance.document_type(), DocumentType::Note);
    assert!(source.content.contains("180 dias"));
    assert!(matches!(
        source.provenance.reference.location,
        SourceLocation::Note { .. }
    ));
    // The label has no page, sheet or file: the note's name and a range of characters.
    assert!(
        source
            .label
            .starts_with("Decisões do módulo de reembolso · caracteres "),
        "{}",
        source.label
    );
    assert!(!source.provenance.reference.previewable());
    // No viewer target exists for a note.
    assert!(
        answer
            .citations
            .iter()
            .filter(|c| c.document_id == note)
            .all(|c| c.viewer_target().is_none())
    );
}

#[tokio::test]
async fn a_note_is_indexed_again_and_removed_without_any_file() {
    let lib = library("note-lifecycle");
    let (id, _) = add(&lib, NOTE).await;
    let before = lib.db.chunks_of(id).await.unwrap();

    // Re-indexing (retry, resume at startup) reads the text from the database.
    assert!(matches!(
        lib.ingestion.reindex(id).await,
        ImportOutcome::Imported { .. }
    ));
    assert_eq!(lib.db.chunks_of(id).await.unwrap().len(), before.len());
    assert!(lib.ingestion.resume().await.is_empty());

    let remover = RemoveDocument {
        documents: lib.db.clone(),
        files: lib.ingestion.files.clone(),
        viewer: None,
    };
    remover.remove(id).await.unwrap();
    assert!(lib.db.get(id).await.unwrap().is_none());
    assert!(lib.ingestion.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn pdfs_and_notes_share_the_library() {
    let mut lib = library("note-and-pdf");
    lib.import(&["text.pdf"]).await;
    let (note, _) = add(&lib, NOTE).await;
    let list = lib.ingestion.list().await.unwrap();
    assert_eq!(list.len(), 2);
    let pdf = list.iter().find(|d| d.id != note).unwrap();
    assert_eq!(pdf.document_type, DocumentType::Pdf);
    assert!(pdf.page_count.is_some_and(|n| n > 0));
    assert_eq!(pdf.status, DocumentStatus::Indexed);
}
