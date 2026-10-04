//! Removing a document over the real adapters (PDFium, SQLite + FTS5 + sqlite-vec, library on
//! disk): its chunks, vectors and file go, the chat history that used it goes, the rest stays,
//! and the same PDF can be imported again.

mod support;

use std::sync::Arc;

use nlmx_application::{
    ports::{CancelFlag, DocumentRepository},
    services::{rag::RagOptions, retriever::RetrieverOptions},
    use_cases::{PageError, RemoveDocument},
};
use nlmx_domain::ingestion::{DocumentStatus, ImportOutcome};
use nlmx_fs_library::FsLibrary;
use nlmx_testing::FakeLlmProvider;
use support::{Library, deterministic_embeddings, fixture, temp_dir};

const PRAZO: &str = "quando termina o prazo de carência";

#[tokio::test]
async fn removing_a_document_leaves_nothing_of_it_behind() {
    let dir = temp_dir("removal");
    let mut library = Library::new(dir.clone(), deterministic_embeddings());
    library.import(&["report.pdf", "text.pdf"]).await;
    let (report, text) = (
        library.documents["report.pdf"],
        library.documents["text.pdf"],
    );
    let report_sha = library.db.get(report).await.unwrap().unwrap().sha256;
    let report_file = dir.join("library").join(format!("{report_sha}.pdf"));
    assert!(report_file.exists());

    let chat = library.chat(
        Arc::new(FakeLlmProvider::available().answering("Resposta [1].")),
        RagOptions::default(),
    );
    let answer = async |conversation, question| {
        let (_, a) = chat.ask(conversation, question).await.unwrap();
        chat.answer(a, &|_| {}, CancelFlag::default())
            .await
            .unwrap()
    };
    // A conversation about the report only, and one that used both documents.
    let scoped = chat.start(Some(report)).await.unwrap().id;
    answer(scoped, "Explique este documento.").await;
    let mixed = chat.start(Some(report)).await.unwrap().id;
    let about_report = answer(mixed, PRAZO).await;
    assert!(about_report.sources.iter().any(|s| s.document_id == report));
    chat.set_scope(mixed, Some(text)).await.unwrap();
    let about_text = answer(mixed, "Explique este documento.").await;
    assert!(about_text.sources.iter().all(|s| s.document_id == text));
    chat.set_scope(mixed, None).await.unwrap();

    let viewer = Arc::new(library.viewer());
    viewer.text_layer(report, 1).await.unwrap(); // cached
    let remover = RemoveDocument {
        documents: library.db.clone(),
        files: Arc::new(FsLibrary::new(dir.join("library"))),
        viewer: Some(viewer.clone()),
    };
    let impact = remover.remove(report).await.unwrap();
    assert!(impact.chunks > 0);
    assert_eq!((impact.conversations, impact.turns), (1, 1));

    // The document, its file and its cached text are gone; the other document is intact.
    assert!(library.db.get(report).await.unwrap().is_none());
    assert!(!report_file.exists());
    assert_eq!(
        viewer.outline(report).await.unwrap_err(),
        PageError::NotFound
    );
    assert_eq!(
        viewer.text_layer(report, 1).await.unwrap_err(),
        PageError::NotFound
    );
    assert_eq!(
        library.db.get(text).await.unwrap().unwrap().status,
        DocumentStatus::Indexed
    );

    // History: the scoped conversation is gone; the mixed one keeps only the turn about the text.
    assert!(chat.conversation(scoped).await.is_err());
    let left = chat.messages(mixed).await.unwrap();
    assert_eq!(left.len(), 2);
    assert_eq!(left[1].id, about_text.id);

    // Neither the lexical nor the vector index returns anything from it.
    let ctx = library
        .retriever()
        .retrieve(
            PRAZO,
            &RetrieverOptions {
                min_score: 0.0,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(ctx.passages.iter().all(|p| p.document_id != report));
    let conn = rusqlite::Connection::open(dir.join("nlmx.sqlite3")).unwrap();
    let rows: i64 = conn
        .query_row(
            "SELECT count(*) FROM chunk_embeddings WHERE document_id = ?1",
            [report],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);

    // The same PDF can be imported again, as a new document.
    let again = library.ingestion.import(&fixture("report.pdf")).await;
    assert!(
        matches!(
            again,
            ImportOutcome::Imported {
                status: DocumentStatus::Indexed,
                ..
            }
        ),
        "{again:?}"
    );
    assert!(report_file.exists());
}
