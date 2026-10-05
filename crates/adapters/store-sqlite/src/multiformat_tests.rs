//! Multi-format persistence (ADR 0013): the migration over existing PDFs, documents of every
//! format, reindexing, removal and the integrity of chunks, sections and embeddings.

use std::path::Path;

use nlmx_application::ports::{
    DocumentRepository, Extraction, InsertOutcome, NewDocument, PageRecord, VectorStore,
};
use nlmx_domain::{
    document::BoundingBox,
    document_type::DocumentType,
    ingestion::{ChunkDraft, DocumentStatus, PageBox},
    parsed::{ChunkContext, DocumentChunk},
    retrieval::{LexicalQuery, PageRange, RetrievalFilter},
    source::SourceLocation,
    vectors::EmbeddingSpace,
};
use nlmx_testing::{document_store_contract, sample_nested_document, sample_stored_extraction};
use rusqlite::{Connection, params};

use crate::{
    Database, LATEST_VERSION, connection, migrations,
    tests::{count, seed, tables, temp_db},
};

fn conn_at(path: &Path) -> Connection {
    connection::open(path).unwrap()
}

fn strings(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(sql).unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn space() -> EmbeddingSpace {
    EmbeddingSpace {
        model_id: "modelo".into(),
        revision: "r1".into(),
        dimensions: 4,
        context_length: 512,
    }
}

/// Problems in the relations between documents, sections, chunks and embeddings.
fn integrity(conn: &Connection) -> Vec<String> {
    let mut problems = Vec::new();
    let mut check = |name: &str, sql: &str| {
        let n = count(conn, sql);
        if n != 0 {
            problems.push(format!("{name}: {n}"));
        }
    };
    check(
        "a chunk of a format other than pdf has no provenance",
        "SELECT count(*) FROM document_chunks c JOIN documents d ON d.id = c.document_id
         LEFT JOIN chunk_provenance p ON p.chunk_id = c.id
         WHERE d.format <> 'pdf' AND p.chunk_id IS NULL",
    );
    check(
        "a chunk of a format other than pdf has a page other than the placeholder",
        "SELECT count(*) FROM document_chunks c JOIN documents d ON d.id = c.document_id
         WHERE d.format <> 'pdf' AND (c.page_start <> 1 OR c.page_end <> 1 OR c.bboxes <> '[]')",
    );
    check(
        "the locator is of another format than the document",
        "SELECT count(*) FROM chunk_provenance p
         JOIN document_chunks c ON c.id = p.chunk_id JOIN documents d ON d.id = c.document_id
         WHERE json_extract(p.locator, '$.kind') <> d.format",
    );
    check(
        "a chunk points to the section of another document",
        "SELECT count(*) FROM chunk_provenance p
         JOIN document_chunks c ON c.id = p.chunk_id
         JOIN document_sections s ON s.id = p.section_id WHERE s.document_id <> c.document_id",
    );
    check(
        "a section has a parent in another document",
        "SELECT count(*) FROM document_sections s
         JOIN document_sections p ON p.id = s.parent_id WHERE p.document_id <> s.document_id",
    );
    for table in strings(
        conn,
        "SELECT name FROM sqlite_schema WHERE name LIKE 'chunk_vectors_%' AND type = 'table'
         AND name NOT LIKE '%\\_%\\_%' ESCAPE '\\'",
    ) {
        let model: i64 = table.trim_start_matches("chunk_vectors_").parse().unwrap();
        check(
            &format!("{table}: a vector of a chunk that does not exist"),
            &format!(
                "SELECT count(*) FROM {table} WHERE rowid NOT IN (SELECT id FROM document_chunks)"
            ),
        );
        check(
            &format!("{table}: a vector without its chunk_embeddings row"),
            &format!(
                "SELECT count(*) FROM {table} WHERE rowid NOT IN
                 (SELECT chunk_id FROM chunk_embeddings WHERE embedding_model_id = {model})"
            ),
        );
        check(
            &format!("{table}: a chunk_embeddings row without its vector"),
            &format!(
                "SELECT count(*) FROM chunk_embeddings WHERE embedding_model_id = {model}
                 AND chunk_id NOT IN (SELECT rowid FROM {table})"
            ),
        );
    }
    problems
}

async fn open() -> (Database, std::path::PathBuf) {
    let path = temp_db();
    (Database::open(&path).unwrap(), path)
}

fn pdf_new(sha: char) -> NewDocument {
    NewDocument {
        sha256: sha.to_string().repeat(64),
        original_filename: "contrato.pdf".into(),
        original_path: "/origem/contrato.pdf".into(),
        library_path: format!("/biblioteca/{sha}.pdf"),
        file_size: 10,
        document_type: DocumentType::Pdf,
    }
}

async fn insert(db: &Database, kind: DocumentType, sha: char) -> i64 {
    let ext = kind.extensions()[0];
    let new = NewDocument {
        sha256: sha.to_string().repeat(64),
        original_filename: format!("arquivo.{ext}"),
        original_path: format!("/origem/arquivo.{ext}"),
        library_path: format!("/biblioteca/{sha}.{ext}"),
        file_size: 10,
        document_type: kind,
    };
    match db.insert(new).await.unwrap() {
        InsertOutcome::Inserted(id) => id,
        other => panic!("{other:?}"),
    }
}

/// Embeds every chunk of a document with a dummy vector.
async fn embed(db: &Database, id: i64) {
    let index = db.create_index(&space()).await.unwrap();
    let items: Vec<(i64, Vec<f32>)> = db
        .chunks_of(id)
        .await
        .unwrap()
        .iter()
        .map(|c| (c.chunk_id.unwrap(), vec![0.1, 0.2, 0.3, 0.4]))
        .collect();
    db.insert_batch(index.id, &items).await.unwrap();
}

// ── Schema ───────────────────────────────────────────────────────────────────

#[test]
fn the_schema_has_the_multi_format_tables_and_columns() {
    let conn = connection::open(&temp_db()).unwrap();
    let mut conn = conn;
    migrations::migrate_to_latest(&mut conn).unwrap();
    assert_eq!(migrations::current_version(&conn).unwrap(), LATEST_VERSION);
    let tables = tables(&conn);
    for table in ["document_sections", "chunk_provenance"] {
        assert!(tables.iter().any(|t| t == table), "missing {table}");
    }
    let columns = strings(&conn, "SELECT name FROM pragma_table_xinfo('documents')");
    for column in [
        "format",
        "mime_type",
        "metadata",
        "normalizer_version",
        "previewable",
    ] {
        assert!(
            columns.iter().any(|c| c == column),
            "missing documents.{column}"
        );
    }
    // A new document is a PDF by default; previewable follows the format.
    conn.execute_batch(
        "INSERT INTO documents (id, sha256, original_filename, library_path, file_size)
             VALUES (1, 'a', 'x', 'l', 1)",
    )
    .unwrap_err(); // sha256 must have 64 characters
    conn.execute(
        "INSERT INTO documents (id, sha256, original_filename, library_path, file_size, format)
         VALUES (1, ?1, 'x.md', 'l', 1, 'markdown'), (2, ?2, 'y.pdf', 'm', 1, 'pdf')",
        params!["a".repeat(64), "b".repeat(64)],
    )
    .unwrap();
    let previewable: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT previewable FROM documents ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(previewable, [0, 1]);
}

#[test]
fn the_new_tables_reject_invalid_states() {
    let mut conn = connection::open(&temp_db()).unwrap();
    migrations::migrate_to_latest(&mut conn).unwrap();
    let sha = "a".repeat(64);
    let rejected = |conn: &Connection, sql: &str| {
        assert!(
            conn.execute_batch(sql).is_err(),
            "should be rejected: {sql}"
        );
    };
    rejected(
        &conn,
        &format!(
            "INSERT INTO documents (sha256, original_filename, library_path, file_size, format)
             VALUES ('{sha}', 'x', 'l', 1, 'odt')"
        ),
    );
    rejected(
        &conn,
        &format!(
            "INSERT INTO documents (sha256, original_filename, library_path, file_size, metadata)
             VALUES ('{sha}', 'x', 'l', 1, 'not json')"
        ),
    );
    conn.execute_batch(&format!(
        "INSERT INTO documents (id, sha256, original_filename, library_path, file_size, format)
         VALUES (1, '{sha}', 'x.md', 'l', 1, 'markdown')"
    ))
    .unwrap();
    for bad in [
        // an unknown kind, a heading without a title, bad JSON, a negative level
        "(1, 'chapter', 'T', 1, 0, '[]', '{}')",
        "(1, 'heading', NULL, 1, 0, '[]', '{}')",
        "(1, 'body', NULL, 0, 0, 'x', '{}')",
        "(1, 'body', NULL, -1, 0, '[]', '{}')",
    ] {
        rejected(
            &conn,
            &format!(
                "INSERT INTO document_sections (document_id, kind, title, level, ordinal, path, locator)
                 VALUES {bad}"
            ),
        );
    }
    conn.execute_batch(
        "INSERT INTO document_sections (id, document_id, kind, title, level, ordinal, path, locator)
         VALUES (1, 1, 'heading', 'T', 1, 0, '[\"T\"]', '{\"kind\":\"markdown\"}')",
    )
    .unwrap();
    // The same ordinal twice in a document.
    rejected(
        &conn,
        "INSERT INTO document_sections (document_id, kind, title, level, ordinal, path, locator)
         VALUES (1, 'body', NULL, 0, 0, '[]', '{}')",
    );
    // Provenance needs a chunk and a valid locator.
    rejected(
        &conn,
        "INSERT INTO chunk_provenance (chunk_id, locator) VALUES (999, '{}')",
    );
}

// ── Migration over existing PDFs ─────────────────────────────────────────────

/// A library as the previous version left it: a PDF with pages, chunks, FTS rows, vectors and
/// a cited answer — then upgraded by opening it with this version.
#[tokio::test]
async fn existing_pdfs_survive_the_migration_untouched() {
    let path = temp_db();
    let snapshot = |conn: &Connection| {
        let mut all = strings(
            conn,
            "SELECT id || '|' || sha256 || '|' || coalesce(title, '') || '|' || original_filename || '|' ||
                    library_path || '|' || file_size || '|' || status || '|' || version || '|' ||
                    created_at || '|' || updated_at || '|' || imported_at FROM documents",
        );
        all.extend(strings(
            conn,
            "SELECT id || '|' || ordinal || '|' || text || '|' || page_start || '|' || page_end || '|' ||
                    coalesce(section_path, '') || '|' || bboxes || '|' || content_hash || '|' || token_count
             FROM document_chunks ORDER BY id",
        ));
        all.extend(strings(
            conn,
            "SELECT message_id || '|' || chunk_id || '|' || document_id || '|' || page_number || '|' || quote
             FROM citations",
        ));
        all.extend(strings(
            conn,
            "SELECT chunk_id || '|' || embedding_model_id || '|' || document_id || '|' || content_hash
             FROM chunk_embeddings",
        ));
        // What a search returns, in order: lexical (BM25) and vector (KNN).
        all.extend(strings(
            conn,
            "SELECT rowid || '|' || round(bm25(document_chunks_fts), 6) FROM document_chunks_fts
             WHERE document_chunks_fts MATCH 'carência OR cobertura' ORDER BY bm25(document_chunks_fts), rowid",
        ));
        all.extend(strings(
            conn,
            "SELECT rowid || '|' || round(distance, 6) FROM chunk_vectors_1
             WHERE embedding MATCH '[0.1,0.2,0.3,0.4]' AND k = 2 ORDER BY distance",
        ));
        all.push(format!(
            "fts={} vec={}",
            count(
                conn,
                "SELECT count(*) FROM document_chunks_fts WHERE document_chunks_fts MATCH 'carência'"
            ),
            count(conn, "SELECT count(*) FROM chunk_vectors_1"),
        ));
        all
    };

    // Version 10, with data.
    let before = {
        let mut conn = conn_at(&path);
        migrations::migrate_to(&mut conn, 10).unwrap();
        seed(&conn);
        drop(conn);
        // The vector table is created at runtime, like the app does.
        let mut conn = conn_at(&path);
        crate::vector::ensure_table(&mut conn, 1).unwrap();
        conn.execute(
            "INSERT INTO chunk_vectors_1 (rowid, embedding, document_id) VALUES (10, '[0.1,0.2,0.3,0.4]', 1),
                                                                               (11, '[0.4,0.3,0.2,0.1]', 1)",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO chunk_embeddings (chunk_id, embedding_model_id, document_id, content_hash)
             VALUES (10, 1, 1, 'h0'), (11, 1, 1, 'h1')",
        )
        .unwrap();
        assert_eq!(migrations::current_version(&conn).unwrap(), 10);
        snapshot(&conn)
    };

    // The app upgrades it by opening it.
    let db = Database::open(&path).unwrap();
    let conn = conn_at(&path);
    assert_eq!(migrations::current_version(&conn).unwrap(), LATEST_VERSION);
    assert_eq!(
        snapshot(&conn),
        before,
        "no PDF row, chunk, citation or vector changed"
    );

    // Defaults: a PDF, previewable, no media type stored, no structure yet.
    let (format, previewable, mime, metadata, normalizer): (String, i64, Option<String>, String, Option<i64>) =
        conn.query_row(
            "SELECT format, previewable, mime_type, metadata, normalizer_version FROM documents WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        (
            format.as_str(),
            previewable,
            mime,
            metadata.as_str(),
            normalizer
        ),
        ("pdf", 1, None, "{}", None)
    );
    assert_eq!(count(&conn, "SELECT count(*) FROM document_sections"), 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM chunk_provenance"), 0);

    // Read through the new API: a PDF document with PDF locations built from the old columns.
    let record = db.get(1).await.unwrap().unwrap();
    assert_eq!(record.document_type, DocumentType::Pdf);
    assert_eq!(record.mime_type, "application/pdf");
    assert!(record.previewable());
    let chunks = db.chunks_of(1).await.unwrap();
    assert_eq!(chunks.len(), 2);
    assert_eq!(
        chunks[0].location,
        SourceLocation::pdf(1, 1, vec![]).unwrap()
    );
    assert_eq!(chunks[0].metadata.document_type, DocumentType::Pdf);
    assert_eq!(chunks[0].metadata.file_name.as_deref(), Some("manual.pdf"));
    assert_eq!(chunks[0].content_hash, "h0");
    assert!(db.sections_of(1).await.unwrap().is_empty());

    // The cited answer of the old version reads as a PDF source: page, quote, no name yet.
    let answer = nlmx_application::ports::ConversationRepository::message(&db, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer.sources.len(), 1);
    let source = &answer.sources[0];
    assert_eq!(source.document_type(), DocumentType::Pdf);
    assert!(source.previewable());
    assert_eq!(
        source.reference.location,
        SourceLocation::pdf(1, 1, vec![]).unwrap()
    );
    assert_eq!(source.reference.chunk_id, Some(10));
    assert_eq!(source.document_name, "");
    assert_eq!(source.quote, "O prazo de carência é de 180 dias.");

    // The runtime vector trigger still follows the chunks.
    conn.execute("DELETE FROM document_chunks WHERE id = 11", [])
        .unwrap();
    assert_eq!(count(&conn, "SELECT count(*) FROM chunk_vectors_1"), 1);
    assert_eq!(count(&conn, "SELECT count(*) FROM chunk_embeddings"), 1);
    assert!(integrity(&conn).is_empty(), "{:?}", integrity(&conn));
    drop(conn);

    // And back to 10 without losing the PDF.
    let mut conn = conn_at(&path);
    migrations::migrate_to(&mut conn, 10).unwrap();
    assert!(
        !tables(&conn)
            .iter()
            .any(|t| t == "document_sections" || t == "chunk_provenance")
    );
    assert_eq!(count(&conn, "SELECT count(*) FROM documents"), 1);
    assert_eq!(count(&conn, "SELECT count(*) FROM document_chunks"), 1);
}

/// Going back from the citation provenance (12) drops only what the old schema cannot hold: a
/// citation of a document that is not a PDF. A PDF citation stays, whole.
#[test]
fn downgrading_citations_keeps_pdf_ones() {
    let mut conn = conn_at(&temp_db());
    migrations::migrate_to_latest(&mut conn).unwrap();
    seed(&conn);
    conn.execute_batch(
        "INSERT INTO documents (id, sha256, original_filename, library_path, file_size, format)
             VALUES (2, 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'dados.csv', 'library/b.csv', 10, 'csv');
         INSERT INTO citations (message_id, ordinal, document_id, page_number, quote, document_type, document_name, locator)
             VALUES (1, 2, 2, 1, 'linha', 'csv', 'dados.csv', '{\"kind\":\"csv\",\"row_start\":3,\"row_end\":4}');",
    )
    .unwrap();
    assert_eq!(count(&conn, "SELECT count(*) FROM citations"), 2);

    migrations::migrate_to(&mut conn, 11).unwrap();
    assert_eq!(migrations::current_version(&conn).unwrap(), 11);
    assert_eq!(count(&conn, "SELECT count(*) FROM citations"), 1);
    let quote: String = conn
        .query_row("SELECT quote FROM citations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(quote, "O prazo de carência é de 180 dias.");
    assert_eq!(
        count(&conn, "SELECT count(*) FROM documents"),
        2,
        "documents are not touched"
    );

    // And forward again: the PDF citation is still there, now with provenance defaults.
    migrations::migrate_to_latest(&mut conn).unwrap();
    let (kind, name, locator): (String, String, Option<String>) = conn
        .query_row(
            "SELECT document_type, document_name, locator FROM citations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((kind.as_str(), name.as_str(), locator), ("pdf", "", None));
}

#[test]
fn downgrading_drops_documents_of_other_formats_and_keeps_pdfs() {
    let mut conn = conn_at(&temp_db());
    migrations::migrate_to_latest(&mut conn).unwrap();
    seed(&conn);
    conn.execute(
        "INSERT INTO documents (id, sha256, original_filename, library_path, file_size, format)
         VALUES (2, ?1, 'x.md', 'l', 1, 'markdown')",
        params!["c".repeat(64)],
    )
    .unwrap();
    conn.execute_batch(
        "INSERT INTO document_chunks (id, document_id, ordinal, text, token_count, page_start, page_end, content_hash)
             VALUES (20, 2, 0, 'texto markdown', 2, 1, 1, 'h');
         INSERT INTO chunk_provenance (chunk_id, locator) VALUES (20, '{\"kind\":\"markdown\"}');",
    )
    .unwrap();
    migrations::migrate_to(&mut conn, 10).unwrap();
    assert_eq!(count(&conn, "SELECT count(*) FROM documents"), 1);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM document_chunks WHERE document_id = 2"
        ),
        0
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM document_chunks_fts WHERE document_chunks_fts MATCH 'markdown'"
        ),
        0
    );
}

// ── Documents of every format ────────────────────────────────────────────────

#[tokio::test]
async fn the_database_honours_the_document_store_contract() {
    let (db, path) = open().await;
    document_store_contract(&db).await;
    assert!(integrity(&conn_at(&path)).is_empty());
}

#[tokio::test]
async fn each_format_is_stored_with_its_location_and_the_placeholder_page() {
    let (db, path) = open().await;
    for (kind, sha) in DocumentType::ALL
        .into_iter()
        .zip(['a', 'b', 'c', 'd', 'e', 'f', 'g'])
    {
        let id = insert(&db, kind, sha).await;
        let stored = sample_stored_extraction(&sample_nested_document(kind), id);
        db.save_processed(id, stored.clone()).await.unwrap();

        let conn = conn_at(&path);
        let (format, previewable, mime): (String, i64, String) = conn
            .query_row(
                "SELECT format, previewable, mime_type FROM documents WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(format, kind.as_str());
        assert_eq!(previewable, i64::from(kind == DocumentType::Pdf), "{kind}");
        assert_eq!(mime, kind.mime_types()[0]);

        let metadata: String = conn
            .query_row("SELECT metadata FROM documents WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        if kind != DocumentType::Csv {
            assert!(metadata.contains("pt-BR"), "{metadata}");
        }
        let (versions, title): (String, String) = conn
            .query_row(
                "SELECT extractor_version || '/' || normalizer_version || '/' || chunker_version, title
                 FROM documents WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(versions, "7/3/5");
        assert_eq!(title, stored.title);

        // Pages and boxes live where they always did, for a PDF; others hold the placeholder.
        let rows: Vec<(i64, i64)> = {
            let mut stmt = conn
                .prepare("SELECT page_start, page_end FROM document_chunks WHERE document_id = ?1 ORDER BY ordinal")
                .unwrap();
            stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        if kind == DocumentType::Pdf {
            assert!(
                rows.iter().any(|&(start, _)| start > 1),
                "real pages are kept"
            );
        } else {
            assert!(rows.iter().all(|&r| r == (1, 1)), "{kind}: {rows:?}");
        }
        // Every chunk is linked to its section (the body section holds a CSV's rows).
        assert_eq!(
            count(
                &conn,
                &format!(
                    "SELECT count(*) FROM chunk_provenance p JOIN document_chunks c ON c.id = p.chunk_id
                     WHERE c.document_id = {id} AND p.section_id IS NULL"
                )
            ),
            0,
            "{kind}"
        );
    }
    assert!(integrity(&conn_at(&path)).is_empty());
}

#[tokio::test]
async fn a_pdf_saved_the_new_way_matches_the_old_way() {
    let (db, path) = open().await;
    let boxes = vec![PageBox {
        page: 2,
        bbox: BoundingBox {
            left: 72.0,
            top: 60.5,
            right: 300.0,
            bottom: 75.25,
        },
    }];
    let draft = ChunkDraft {
        index: 0,
        text: "O prazo de carência é de 180 dias.".into(),
        token_count: 9,
        page_start: 2,
        page_end: 3,
        section_path: vec!["1 Contrato".into(), "1.1 Prazos".into()],
        boxes: boxes.clone(),
        content_hash: "h0".into(),
    };
    let old = insert(&db, DocumentType::Pdf, 'a').await;
    db.save_extraction(
        old,
        Extraction {
            title: "Contrato".into(),
            author: None,
            pdf_created_at: None,
            page_count: 3,
            has_text_layer: true,
            pages: vec![PageRecord {
                number: 1,
                width: 612.0,
                height: 792.0,
                char_count: 10,
                has_text: true,
            }],
            chunks: vec![draft.clone()],
            extractor_version: 1,
            chunker_version: 1,
            status: DocumentStatus::Embedding,
        },
    )
    .await
    .unwrap();

    let new = insert(&db, DocumentType::Pdf, 'b').await;
    let context = ChunkContext {
        document_id: new,
        document_title: Some("Contrato".into()),
        file_name: Some("contrato.pdf".into()),
        language: None,
    };
    let mut stored = sample_stored_extraction(&sample_nested_document(DocumentType::Pdf), new);
    stored.chunks = vec![DocumentChunk::from_draft(&draft, &context)];
    stored.outline.clear();
    db.save_processed(new, stored).await.unwrap();

    let conn = conn_at(&path);
    let columns = |id: i64| {
        strings(
            &conn,
            &format!(
                "SELECT ordinal || '|' || text || '|' || token_count || '|' || page_start || '|' || page_end || '|' ||
                        coalesce(section_path, '') || '|' || bboxes || '|' || content_hash
                 FROM document_chunks WHERE document_id = {id}"
            ),
        )
    };
    assert_eq!(
        columns(old),
        columns(new),
        "the viewer and the citations read the same columns"
    );

    // Both read back the same way.
    let a = db.chunks_of(old).await.unwrap();
    let b = db.chunks_of(new).await.unwrap();
    assert_eq!(a[0].location, b[0].location);
    assert_eq!(b[0].location, SourceLocation::pdf(2, 3, boxes).unwrap());
    assert_eq!(a[0].section_path, ["1 Contrato", "1.1 Prazos"]);
}

// ── Idempotence, reindexing, removal, embeddings ─────────────────────────────

#[tokio::test]
async fn importing_and_saving_again_never_duplicates() {
    let (db, path) = open().await;
    let id = insert(&db, DocumentType::Markdown, 'a').await;
    let again = db
        .insert(NewDocument {
            original_filename: "outro-nome.txt".into(),
            document_type: DocumentType::Text,
            ..pdf_new('a')
        })
        .await
        .unwrap();
    assert_eq!(again, InsertOutcome::AlreadyExists(id));
    let conn = conn_at(&path);
    assert_eq!(count(&conn, "SELECT count(*) FROM documents"), 1);
    assert_eq!(count(&conn, "SELECT count(*) FROM embedding_jobs"), 1);
    assert_eq!(
        strings(&conn, "SELECT format FROM documents"),
        ["markdown"],
        "the first import decides the format"
    );

    let stored = sample_stored_extraction(&sample_nested_document(DocumentType::Markdown), id);
    for _ in 0..3 {
        db.save_processed(id, stored.clone()).await.unwrap();
    }
    assert_eq!(
        count(&conn, "SELECT count(*) FROM document_chunks"),
        stored.chunks.len() as i64
    );
    assert_eq!(
        count(&conn, "SELECT count(*) FROM chunk_provenance"),
        stored.chunks.len() as i64
    );
    assert_eq!(
        count(&conn, "SELECT count(*) FROM document_sections"),
        stored.outline.len() as i64
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM document_chunks_fts WHERE document_chunks_fts MATCH 'Parágrafo'"
        ),
        stored.chunks.len() as i64,
        "no duplicate FTS rows"
    );
    assert!(integrity(&conn).is_empty());
}

#[tokio::test]
async fn reindexing_replaces_chunks_embeddings_and_vectors_consistently() {
    let (db, path) = open().await;
    let id = insert(&db, DocumentType::Epub, 'a').await;
    let other = insert(&db, DocumentType::Text, 'b').await;
    let stored = sample_stored_extraction(&sample_nested_document(DocumentType::Epub), id);
    db.save_processed(id, stored.clone()).await.unwrap();
    let kept = sample_stored_extraction(&sample_nested_document(DocumentType::Text), other);
    db.save_processed(other, kept.clone()).await.unwrap();
    embed(&db, id).await;
    embed(&db, other).await;

    let conn = conn_at(&path);
    let table = "chunk_vectors_1";
    let total = (stored.chunks.len() + kept.chunks.len()) as i64;
    assert_eq!(
        count(&conn, &format!("SELECT count(*) FROM {table}")),
        total
    );
    assert_eq!(count(&conn, "SELECT count(*) FROM chunk_embeddings"), total);
    assert!(integrity(&conn).is_empty(), "{:?}", integrity(&conn));

    // Reindexing the first document drops its old vectors with its old chunks, and only those.
    let old_ids: Vec<i64> = db
        .chunks_of(id)
        .await
        .unwrap()
        .iter()
        .map(|c| c.chunk_id.unwrap())
        .collect();
    let mut changed = stored.clone();
    changed.chunks.truncate(3);
    db.save_processed(id, changed.clone()).await.unwrap();
    let new_ids: Vec<i64> = db
        .chunks_of(id)
        .await
        .unwrap()
        .iter()
        .map(|c| c.chunk_id.unwrap())
        .collect();
    assert_eq!(new_ids.len(), 3);
    assert!(
        new_ids.iter().all(|n| !old_ids.contains(n)),
        "chunks are replaced, ids are not reused"
    );
    assert_eq!(
        count(&conn, &format!("SELECT count(*) FROM {table}")),
        kept.chunks.len() as i64,
        "the other document keeps its vectors"
    );
    assert_eq!(
        count(&conn, "SELECT count(*) FROM chunk_embeddings"),
        kept.chunks.len() as i64
    );
    assert!(integrity(&conn).is_empty(), "{:?}", integrity(&conn));

    // Embedding the new chunks restores one vector per chunk.
    embed(&db, id).await;
    assert_eq!(
        count(&conn, &format!("SELECT count(*) FROM {table}")),
        (3 + kept.chunks.len()) as i64
    );
    assert!(integrity(&conn).is_empty(), "{:?}", integrity(&conn));

    // Removing a document removes its vectors, structure and provenance, and no one else's.
    db.remove(id).await.unwrap().unwrap();
    assert_eq!(
        count(&conn, &format!("SELECT count(*) FROM {table}")),
        kept.chunks.len() as i64
    );
    assert_eq!(
        count(&conn, "SELECT count(*) FROM chunk_embeddings"),
        kept.chunks.len() as i64
    );
    assert_eq!(
        count(
            &conn,
            &format!("SELECT count(*) FROM document_sections WHERE document_id = {id}")
        ),
        0
    );
    assert_eq!(
        count(&conn, "SELECT count(*) FROM chunk_provenance"),
        kept.chunks.len() as i64
    );
    assert_eq!(
        count(
            &conn,
            &format!("SELECT count(*) FROM document_chunks WHERE document_id = {id}")
        ),
        0
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM document_chunks_fts WHERE document_chunks_fts MATCH 'Parágrafo'"
        ),
        kept.chunks.len() as i64
    );
    assert!(integrity(&conn).is_empty(), "{:?}", integrity(&conn));
}

#[tokio::test]
async fn a_removed_document_leaves_no_text_in_the_database_bytes() {
    let (db, path) = open().await;
    let id = insert(&db, DocumentType::Markdown, 'a').await;
    let mut stored = sample_stored_extraction(&sample_nested_document(DocumentType::Markdown), id);
    stored.chunks[0].text = "MARCADOR-SECRETO-QUE-DEVE-SUMIR conteúdo do trecho".into();
    stored.outline[0].title = Some("TITULO-SECRETO-DA-SECAO".into());
    stored.outline[0].path = vec!["TITULO-SECRETO-DA-SECAO".into()];
    stored.chunks[0].section_path = vec!["TITULO-SECRETO-DA-SECAO".into()];
    db.save_processed(id, stored).await.unwrap();
    db.remove(id).await.unwrap().unwrap();
    drop(db);

    // Fold the WAL into the main file, then look at every byte of both.
    {
        let conn = conn_at(&path);
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }
    for file in [path.clone(), path.with_extension("sqlite3-wal")] {
        let bytes = std::fs::read(&file).unwrap_or_default();
        for needle in ["MARCADOR-SECRETO-QUE-DEVE-SUMIR", "TITULO-SECRETO-DA-SECAO"] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                "{needle} is still in {}",
                file.display()
            );
        }
    }
}

#[tokio::test]
async fn the_page_filter_only_matches_pdf_chunks() {
    let (db, _) = open().await;
    let pdf = insert(&db, DocumentType::Pdf, 'a').await;
    let md = insert(&db, DocumentType::Markdown, 'b').await;
    for (id, kind) in [(pdf, DocumentType::Pdf), (md, DocumentType::Markdown)] {
        let mut stored = sample_stored_extraction(&sample_nested_document(kind), id);
        for chunk in &mut stored.chunks {
            chunk.text = format!("{} carência", chunk.text);
        }
        db.save_processed(id, stored).await.unwrap();
    }
    use nlmx_application::ports::LexicalIndex;
    let query = LexicalQuery::from_query("carência");
    let everywhere = LexicalIndex::search(&db, &query, 50, &RetrievalFilter::default())
        .await
        .unwrap();
    let in_documents = |hits: &[nlmx_domain::retrieval::LexicalCandidate], id: i64| {
        hits.iter().filter(|h| h.document_id == id).count()
    };
    assert!(in_documents(&everywhere, pdf) > 0 && in_documents(&everywhere, md) > 0);

    // Page 1 exists in the Markdown only as a placeholder: it must not match.
    let page_one = RetrievalFilter {
        pages: Some(PageRange { from: 1, to: 3 }),
        ..Default::default()
    };
    let hits = LexicalIndex::search(&db, &query, 50, &page_one)
        .await
        .unwrap();
    assert_eq!(in_documents(&hits, md), 0, "a Markdown chunk has no page");
    assert!(
        in_documents(&hits, pdf) > 0,
        "a PDF chunk on page 1 still matches"
    );
}

// ── Migration 0013 (DOCX and XLSX) ───────────────────────────────────────────

#[test]
fn migration_0013_accepts_docx_and_xlsx_and_leaves_existing_rows_alone() {
    let mut conn = connection::open(&temp_db()).unwrap();
    migrations::migrate_to(&mut conn, 12).unwrap();
    conn.execute(
        "INSERT INTO documents (id, sha256, original_filename, library_path, file_size, format)
         VALUES (1, ?1, 'a.pdf', 'l', 1, 'pdf'), (2, ?2, 'b.md', 'm', 1, 'markdown')",
        params!["a".repeat(64), "b".repeat(64)],
    )
    .unwrap();
    // Make the rows distinguishable from what a touch would write.
    conn.execute_batch("UPDATE documents SET status = status, version = 5, updated_at = '2020-01-01T00:00:00.000Z'")
        .unwrap();
    let before: Vec<(i64, i64, String, String)> = {
        let mut stmt = conn
            .prepare("SELECT id, version, updated_at, format FROM documents ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };

    migrations::migrate_to_latest(&mut conn).unwrap();

    let after: Vec<(i64, i64, String, String)> = {
        let mut stmt = conn
            .prepare("SELECT id, version, updated_at, format FROM documents ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(after, before, "the copy touches no row");

    for (id, format) in [(3, "docx"), (4, "xlsx")] {
        conn.execute(
            "INSERT INTO documents (id, sha256, original_filename, library_path, file_size, format)
             VALUES (?1, ?2, 'x', 'l', 1, ?3)",
            params![id, format!("{id}").repeat(64), format],
        )
        .unwrap();
    }
    assert!(
        conn.execute(
            "INSERT INTO documents (id, sha256, original_filename, library_path, file_size, format)
             VALUES (5, ?1, 'x', 'l', 1, 'odt')",
            params!["5".repeat(64)],
        )
        .is_err(),
        "other formats are still rejected"
    );
    let previewable: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT previewable FROM documents ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(previewable, [1, 0, 0, 0]);
    // The touch trigger is back.
    conn.execute(
        "UPDATE documents SET original_filename = 'y' WHERE id = 1",
        [],
    )
    .unwrap();
    let version: i64 = conn
        .query_row("SELECT version FROM documents WHERE id = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(version, 6);
    assert!(
        strings(&conn, "PRAGMA integrity_check") == ["ok"]
            && conn
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .query_map([], |_| Ok(()))
                .unwrap()
                .count()
                == 0
    );

    // Downgrading drops the new formats and keeps the rest.
    migrations::migrate_to(&mut conn, 12).unwrap();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM documents"), 2);
    assert!(
        conn.execute(
            "INSERT INTO documents (id, sha256, original_filename, library_path, file_size, format)
             VALUES (9, ?1, 'x', 'l', 1, 'docx')",
            params!["9".repeat(64)],
        )
        .is_err()
    );
}
