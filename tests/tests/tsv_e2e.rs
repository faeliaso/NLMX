//! TSV is the CSV pipeline with the tab as delimiter: import, chunks located by row, embeddings,
//! lexical search, reindexing, a changed file and removal, with every real adapter.

mod support;

use nlmx_application::ports::DocumentRepository;
use nlmx_domain::{
    document_type::DocumentType,
    ingestion::{DocumentStatus, ImportOutcome},
    source::SourceLocation,
};
use rusqlite::params;
use support::multiformat::{App, a_word_of, imported, text_fixture};

#[tokio::test]
async fn a_tsv_is_imported_chunked_by_row_embedded_and_found() {
    let app = App::new("tsv-import", true);
    let path = app.user_file(&text_fixture("funcionarios.tsv"), "funcionarios.tsv");
    let (id, chunks, status) = imported(app.ingestion.import(&path).await);
    assert_eq!(status, DocumentStatus::Indexed);
    assert!(chunks > 0);

    let record = app.db.get(id).await.unwrap().unwrap();
    assert_eq!(record.document_type, DocumentType::Csv);
    assert!(!record.document_type.previewable());
    assert!(
        app.library_files().iter().any(|f| f.ends_with(".tsv")),
        "the library keeps <sha>.tsv"
    );

    let stored = app.db.chunks_of(id).await.unwrap();
    for chunk in &stored {
        assert!(chunk.text.contains("Arquivo: funcionarios.tsv"));
        assert!(chunk.text.contains("Colunas: Nome, Cidade, Idade"));
        assert!(matches!(chunk.location, SourceLocation::Csv { .. }));
    }
    // Rows 1..=4 are covered; the label is the CSV one.
    let first = &stored[0].location;
    assert_eq!(first.document_type(), DocumentType::Csv);
    assert!(first.label().starts_with("linha"), "{}", first.label());

    let embedded: i64 = app.count(&format!(
        "SELECT count(*) FROM chunk_embeddings e JOIN document_chunks c ON c.id = e.chunk_id WHERE c.document_id = {id}"
    ));
    assert_eq!(embedded as u32, chunks);
    let word = a_word_of(&stored[0].text);
    assert!(app.hits(&word).await.contains(&id));
    assert!(app.hits("Fortaleza").await.contains(&id));
}

#[tokio::test]
async fn reindexing_a_tsv_changes_nothing_and_a_changed_file_updates_the_same_document() {
    let app = App::new("tsv-reindex", true);
    let path = app.user_file(&text_fixture("funcionarios.tsv"), "funcionarios.tsv");
    let (id, chunks, _) = imported(app.ingestion.import(&path).await);
    let sizes = app.index_sizes();

    let (again, again_chunks, status) = imported(app.ingestion.reindex(id).await);
    assert_eq!(
        (again, again_chunks, status),
        (id, chunks, DocumentStatus::Indexed)
    );
    assert_eq!(app.index_sizes(), sizes);

    std::fs::write(
        &path,
        "Nome\tCidade\tIdade\nPaulo\tSobral\t50\nLia\tCrato\t23\n",
    )
    .unwrap();
    match app.ingestion.import(&path).await {
        ImportOutcome::Imported { id: same, .. } => assert_eq!(same, id),
        other => panic!("{other:?}"),
    }
    assert!(app.hits("Sobral").await.contains(&id));
    assert!(!app.hits("Fortaleza").await.contains(&id));
}

#[tokio::test]
async fn removing_a_tsv_takes_chunks_vectors_and_index_entries_with_it() {
    let app = App::new("tsv-remove", true);
    let path = app.user_file(&text_fixture("funcionarios.tsv"), "funcionarios.tsv");
    let (id, _, _) = imported(app.ingestion.import(&path).await);
    app.db.remove(id).await.unwrap().unwrap();
    for table in ["document_chunks", "chunk_embeddings", "chunk_provenance"] {
        let n: i64 = app
            .sql()
            .query_row(&format!("SELECT count(*) FROM {table}"), params![], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 0, "{table}");
    }
    assert!(app.hits("Fortaleza").await.is_empty());
}
