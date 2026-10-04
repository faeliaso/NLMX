//! Schema, migration and sqlite-vec tests against real database files.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
};

use nlmx_application::ports::{SettingsRepository, StorageDiagnostics};
use rusqlite::{Connection, params};

use crate::{Database, LATEST_VERSION, connection, migrations};

fn temp_db() -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "nlmx-store-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("test.sqlite3")
}

fn migrated() -> Connection {
    let mut conn = connection::open(&temp_db()).unwrap();
    migrations::migrate_to_latest(&mut conn).unwrap();
    conn
}

fn tables(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap();
    stmt.query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).unwrap()
}

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// One document with a page, two chunks, a collection link, a job and a cited answer.
fn seed(conn: &Connection) -> i64 {
    conn.execute_batch(&format!(
        "INSERT INTO documents (id, sha256, original_filename, library_path, file_size)
             VALUES (1, '{SHA}', 'manual.pdf', 'library/{SHA}.pdf', 1024);
         INSERT INTO document_pages (document_id, page_number, width, height) VALUES (1, 1, 612, 792);
         INSERT INTO document_chunks (id, document_id, ordinal, text, token_count, page_start, page_end, content_hash)
             VALUES (10, 1, 0, 'O prazo de carência é de 180 dias.', 9, 1, 1, 'h0'),
                    (11, 1, 1, 'Cobertura ambulatorial e hospitalar.', 5, 1, 1, 'h1');
         INSERT INTO collections (id, name) VALUES (1, 'Contratos');
         INSERT INTO collection_documents (collection_id, document_id) VALUES (1, 1);
         INSERT INTO embedding_models (id, model_key, revision, display_name, dims, max_tokens, status)
             VALUES (1, 'test-model', 'r1', 'Test', 4, 512, 'installed');
         INSERT INTO embedding_jobs (document_id, embedding_model_id, kind) VALUES (1, 1, 'ingest');
         INSERT INTO conversations (id, title, collection_id) VALUES (1, 'Dúvidas', 1);
         INSERT INTO messages (id, conversation_id, role, content) VALUES (1, 1, 'assistant', 'São 180 dias [1].');
         INSERT INTO citations (message_id, ordinal, chunk_id, document_id, page_number, quote)
             VALUES (1, 1, 10, 1, 1, 'O prazo de carência é de 180 dias.');"
    ))
    .unwrap();
    1
}

#[test]
fn migrations_are_valid() {
    migrations::migrations().validate().unwrap();
}

#[test]
fn migrates_to_latest_and_creates_every_table() {
    let conn = migrated();
    assert_eq!(migrations::current_version(&conn).unwrap(), LATEST_VERSION);
    let tables = tables(&conn);
    for expected in [
        "app_settings",
        "chunk_embeddings",
        "citations",
        "collection_documents",
        "collections",
        "conversations",
        "document_chunks",
        "document_chunks_fts",
        "document_pages",
        "documents",
        "embedding_jobs",
        "embedding_models",
        "messages",
    ] {
        assert!(
            tables.iter().any(|t| t == expected),
            "missing table {expected}"
        );
    }
}

#[test]
fn migrations_are_reversible() {
    let mut conn = migrated();
    seed(&conn);
    migrations::migrate_to(&mut conn, 0).unwrap();
    assert_eq!(migrations::current_version(&conn).unwrap(), 0);
    assert!(
        tables(&conn).is_empty(),
        "down migrations left {:?}",
        tables(&conn)
    );

    migrations::migrate_to_latest(&mut conn).unwrap();
    assert_eq!(migrations::current_version(&conn).unwrap(), LATEST_VERSION);

    // Step down one version at a time and back up.
    for version in (0..LATEST_VERSION).rev() {
        migrations::migrate_to(&mut conn, version).unwrap();
    }
    migrations::migrate_to_latest(&mut conn).unwrap();
}

/// Conversations from before the free chat (0010) keep answering from the documents.
#[test]
fn free_chat_migration_keeps_existing_conversations_on_the_documents() {
    let mut conn = connection::open(&temp_db()).unwrap();
    migrations::migrate_to(&mut conn, 9).unwrap();
    conn.execute_batch(
        "INSERT INTO conversations (id) VALUES (1);
         INSERT INTO messages (conversation_id, role, content) VALUES (1, 'user', 'q');
         INSERT INTO messages (conversation_id, role, content) VALUES (1, 'assistant', 'a');",
    )
    .unwrap();
    migrations::migrate_to_latest(&mut conn).unwrap();
    let mode: String = conn
        .query_row("SELECT mode FROM conversations WHERE id = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(mode, "documents");
    let grounding = |role: &str| -> Option<String> {
        conn.query_row(
            "SELECT grounding FROM messages WHERE role = ?1",
            [role],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(grounding("user"), None);
    assert_eq!(grounding("assistant").as_deref(), Some("documents"));
    assert!(
        conn.execute("UPDATE conversations SET mode = 'outro' WHERE id = 1", [])
            .is_err()
    );
    assert!(
        conn.execute("UPDATE messages SET grounding = 'outro'", [])
            .is_err()
    );
}

#[test]
fn connection_pragmas() {
    let conn = migrated();
    let fk: i64 = conn
        .pragma_query_value(None, "foreign_keys", |r| r.get(0))
        .unwrap();
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(fk, 1);
    assert_eq!(mode, "wal");
}

#[test]
fn deleting_a_document_cascades_to_everything_derived_from_it() {
    let conn = migrated();
    let doc = seed(&conn);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM document_chunks_fts WHERE document_chunks_fts MATCH 'carencia'"
        ),
        1
    );

    conn.execute("DELETE FROM documents WHERE id = ?1", [doc])
        .unwrap();

    for table in [
        "document_pages",
        "document_chunks",
        "collection_documents",
        "embedding_jobs",
        "citations",
    ] {
        assert_eq!(
            count(&conn, &format!("SELECT count(*) FROM {table}")),
            0,
            "{table} not cleaned"
        );
    }
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM document_chunks_fts WHERE document_chunks_fts MATCH 'carencia'"
        ),
        0
    );
    // The answer itself and the collection survive.
    assert_eq!(count(&conn, "SELECT count(*) FROM messages"), 1);
    assert_eq!(count(&conn, "SELECT count(*) FROM collections"), 1);
}

#[test]
fn reindexing_keeps_citation_snapshots() {
    let conn = migrated();
    seed(&conn);
    conn.execute("DELETE FROM document_chunks WHERE document_id = 1", [])
        .unwrap();
    let (chunk, quote): (Option<i64>, String) = conn
        .query_row("SELECT chunk_id, quote FROM citations", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(chunk, None);
    assert!(quote.contains("180 dias"));
}

#[test]
fn removing_a_collection_keeps_its_conversations() {
    let conn = migrated();
    seed(&conn);
    conn.execute("DELETE FROM collections WHERE id = 1", [])
        .unwrap();
    let collection: Option<i64> = conn
        .query_row("SELECT collection_id FROM conversations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(collection, None);
    assert_eq!(count(&conn, "SELECT count(*) FROM documents"), 1);
}

#[test]
fn updates_bump_version_and_updated_at() {
    let conn = migrated();
    seed(&conn);
    conn.execute(
        "UPDATE documents SET updated_at = '2000-01-01T00:00:00.000Z' WHERE id = 1",
        [],
    )
    .unwrap();
    let (v1, _): (i64, String) = conn
        .query_row("SELECT version, updated_at FROM documents", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();

    conn.execute(
        "UPDATE documents SET status = 'extracting' WHERE id = 1",
        [],
    )
    .unwrap();
    let (v2, updated): (i64, String) = conn
        .query_row("SELECT version, updated_at FROM documents", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(v2, v1 + 1);
    assert_ne!(updated, "2000-01-01T00:00:00.000Z");

    // Optimistic concurrency: a stale version updates nothing.
    let changed = conn
        .execute(
            "UPDATE documents SET title = 'x', version = version + 1 WHERE id = 1 AND version = ?1",
            [v1],
        )
        .unwrap();
    assert_eq!(changed, 0);
}

#[test]
fn constraints_reject_invalid_states() {
    let conn = migrated();
    seed(&conn);
    assert!(
        conn.execute("UPDATE documents SET status = 'bogus' WHERE id = 1", [])
            .is_err()
    );
    assert!(
        conn.execute("UPDATE documents SET status = 'failed' WHERE id = 1", [])
            .is_err(),
        "failed needs error"
    );
    assert!(
        conn.execute("UPDATE embedding_jobs SET progress = 1.5", [])
            .is_err()
    );
    assert!(
        conn.execute(
            "INSERT INTO app_settings (key, value) VALUES ('k', 'not json')",
            []
        )
        .is_err()
    );
    assert!(
        conn.execute("INSERT INTO collections (name) VALUES ('CONTRATOS')", [])
            .is_err(),
        "names are case-insensitive unique"
    );

    conn.execute("UPDATE embedding_models SET is_active = 1 WHERE id = 1", [])
        .unwrap();
    conn.execute(
        "INSERT INTO embedding_models (model_key, revision, display_name, dims, max_tokens, status)
         VALUES ('other', 'r1', 'Other', 8, 512, 'installed')",
        [],
    )
    .unwrap();
    assert!(
        conn.execute(
            "UPDATE embedding_models SET is_active = 1 WHERE model_key = 'other'",
            []
        )
        .is_err(),
        "only one active model"
    );
    assert!(
        conn.execute(
            "INSERT INTO embedding_models (model_key, revision, display_name, dims, max_tokens, is_active)
             VALUES ('x', 'r', 'X', 8, 512, 1)",
            [],
        )
        .is_err(),
        "an active model must be installed"
    );
}

#[tokio::test]
async fn database_opens_migrates_and_reports_diagnostics() {
    let path = temp_db();
    let db = Database::open(&path).unwrap();
    let info = db.info().await.unwrap();
    assert_eq!(info.schema_version, LATEST_VERSION);
    assert_eq!(info.latest_schema_version, LATEST_VERSION);
    assert!(
        info.vector_extension_version.starts_with('v'),
        "{}",
        info.vector_extension_version
    );
    assert!(info.path.ends_with("test.sqlite3"));

    // Reopening an up-to-date database is a no-op.
    drop(db);
    Database::open(&path).unwrap();
}

#[tokio::test]
async fn settings_round_trip() {
    let db = Database::open(temp_db()).unwrap();
    assert_eq!(db.get("theme").await.unwrap(), None);
    db.set("theme", r#""dark""#).await.unwrap();
    db.set("theme", r#""light""#).await.unwrap();
    assert_eq!(
        db.get("theme").await.unwrap().as_deref(),
        Some(r#""light""#)
    );
    assert!(db.set("broken", "{").await.is_err());
}

#[tokio::test]
async fn vector_tables_follow_the_embedding_model_and_its_chunks() {
    let path = temp_db();
    let db = Database::open(&path).unwrap();
    {
        let conn = connection::open(&path).unwrap();
        seed(&conn);
    }

    let table = db.ensure_vector_table(1).await.unwrap();
    assert_eq!(table, "chunk_vectors_1");
    assert_eq!(
        db.ensure_vector_table(1).await.unwrap(),
        table,
        "idempotent"
    );

    let conn = connection::open(&path).unwrap();
    let stored: String = conn
        .query_row(
            "SELECT vector_table FROM embedding_models WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, table);

    conn.execute(
        &format!("INSERT INTO {table} (rowid, embedding, document_id) VALUES (?1, ?2, ?3)"),
        params![10, "[0.1, 0.2, 0.3, 0.4]", 1],
    )
    .unwrap();
    assert!(
        conn.execute(
            &format!(
                "INSERT INTO {table} (rowid, embedding, document_id) VALUES (11, '[1, 2]', 1)"
            ),
            []
        )
        .is_err(),
        "dimension is enforced"
    );
    assert_eq!(count(&conn, &format!("SELECT count(*) FROM {table}")), 1);

    // Deleting the chunk deletes its vector.
    conn.execute("DELETE FROM document_chunks WHERE id = 10", [])
        .unwrap();
    assert_eq!(count(&conn, &format!("SELECT count(*) FROM {table}")), 0);
    drop(conn);

    db.drop_vector_table(1).await.unwrap();
    let conn = connection::open(&path).unwrap();
    assert!(!tables(&conn).iter().any(|t| t == &table));
    let stored: Option<String> = conn
        .query_row(
            "SELECT vector_table FROM embedding_models WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, None);
}

mod documents {
    use nlmx_application::ports::{
        DocumentRepository, Extraction, InsertOutcome, NewDocument, PageRecord,
    };
    use nlmx_domain::{
        document::BoundingBox,
        ingestion::{ChunkDraft, DocumentStatus, PageBox},
    };

    use super::{count, temp_db};
    use crate::{Database, connection};

    fn new_doc(sha: char) -> NewDocument {
        NewDocument {
            sha256: sha.to_string().repeat(64),
            original_filename: "contrato.pdf".into(),
            original_path: "/Users/x/contrato.pdf".into(),
            library_path: format!("/lib/{}.pdf", sha.to_string().repeat(64)),
            file_size: 2048,
        }
    }

    fn chunk(index: u32, text: &str) -> ChunkDraft {
        ChunkDraft {
            index,
            text: text.into(),
            token_count: 5,
            page_start: 1,
            page_end: 2,
            section_path: vec!["Contrato".into(), "Cláusula 1".into()],
            boxes: vec![PageBox {
                page: 1,
                bbox: BoundingBox {
                    left: 72.0,
                    top: 55.5,
                    right: 300.0,
                    bottom: 70.25,
                },
            }],
            content_hash: format!("{index:064}"),
        }
    }

    fn extraction(chunks: Vec<ChunkDraft>) -> Extraction {
        Extraction {
            title: "Contrato".into(),
            author: Some("Equipe".into()),
            pdf_created_at: Some("2026-01-02T03:04:05Z".into()),
            page_count: 2,
            has_text_layer: true,
            pages: vec![
                PageRecord {
                    number: 1,
                    width: 612.0,
                    height: 792.0,
                    char_count: 120,
                    has_text: true,
                },
                PageRecord {
                    number: 2,
                    width: 612.0,
                    height: 792.0,
                    char_count: 0,
                    has_text: false,
                },
            ],
            chunks,
            extractor_version: 1,
            chunker_version: 1,
            status: DocumentStatus::Embedding,
        }
    }

    #[tokio::test]
    async fn rebase_library_paths_moves_only_paths_under_the_old_directory() {
        let db = Database::open(temp_db()).unwrap();
        let mut under_old = new_doc('a');
        under_old.library_path = "/data/old_dir/library/a.pdf".into();
        let mut sibling = new_doc('b');
        sibling.library_path = "/data/old_dir2/library/b.pdf".into();
        let mut elsewhere = new_doc('c');
        elsewhere.library_path = "/lib/c.pdf".into();
        let mut ids = Vec::new();
        for doc in [under_old, sibling, elsewhere] {
            let InsertOutcome::Inserted(id) = db.insert(doc).await.unwrap() else {
                panic!("insert")
            };
            ids.push(id);
        }

        let changed = db
            .rebase_library_paths("/data/old_dir".as_ref(), "/data/new dir".as_ref())
            .unwrap();

        assert_eq!(changed, 1);
        let paths = library_paths(&db, &ids).await;
        assert_eq!(
            paths,
            [
                "/data/new dir/library/a.pdf",
                "/data/old_dir2/library/b.pdf",
                "/lib/c.pdf"
            ]
        );
    }

    async fn library_paths(db: &Database, ids: &[i64]) -> Vec<String> {
        let mut paths = Vec::new();
        for id in ids {
            paths.push(db.get(*id).await.unwrap().unwrap().library_path);
        }
        paths
    }

    #[tokio::test]
    async fn insert_detects_duplicates_by_hash_and_creates_one_job() {
        let path = temp_db();
        let db = Database::open(&path).unwrap();
        let InsertOutcome::Inserted(id) = db.insert(new_doc('a')).await.unwrap() else {
            panic!("first insert")
        };
        assert_eq!(
            db.insert(new_doc('a')).await.unwrap(),
            InsertOutcome::AlreadyExists(id)
        );
        assert_eq!(db.find_by_sha256(&"a".repeat(64)).await.unwrap(), Some(id));
        assert_eq!(db.find_by_sha256(&"b".repeat(64)).await.unwrap(), None);

        let conn = connection::open(&path).unwrap();
        assert_eq!(count(&conn, "SELECT count(*) FROM documents"), 1);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM embedding_jobs WHERE kind = 'ingest' AND stage = 'queued'"
            ),
            1
        );

        let record = db.get(id).await.unwrap().unwrap();
        assert_eq!(record.status, DocumentStatus::Queued);
        assert_eq!(db.unfinished().await.unwrap(), vec![id]);
    }

    #[tokio::test]
    async fn save_extraction_replaces_pages_chunks_and_fts_atomically() {
        let path = temp_db();
        let db = Database::open(&path).unwrap();
        let InsertOutcome::Inserted(id) = db.insert(new_doc('c')).await.unwrap() else {
            panic!()
        };

        db.save_extraction(
            id,
            extraction(vec![chunk(0, "carência inicial"), chunk(1, "cobertura")]),
        )
        .await
        .unwrap();
        db.save_extraction(id, extraction(vec![chunk(0, "carência revisada")]))
            .await
            .unwrap();

        let conn = connection::open(&path).unwrap();
        assert_eq!(count(&conn, "SELECT count(*) FROM document_pages"), 2);
        assert_eq!(count(&conn, "SELECT count(*) FROM document_chunks"), 1);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM document_chunks_fts WHERE document_chunks_fts MATCH 'inicial'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM document_chunks_fts WHERE document_chunks_fts MATCH 'revisada'"
            ),
            1
        );
        let (section, bboxes, page_end): (String, String, i64) = conn
            .query_row(
                "SELECT section_path, bboxes, page_end FROM document_chunks",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(section, "Contrato > Cláusula 1");
        assert_eq!(
            bboxes,
            r#"[{"page":1,"left":72.00,"top":55.50,"right":300.00,"bottom":70.25}]"#
        );
        assert_eq!(page_end, 2);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM embedding_jobs WHERE stage = 'waiting_model'"
            ),
            1,
            "job waits for an embedding model"
        );

        let docs = db.list().await.unwrap();
        assert_eq!(
            (
                docs[0].title.as_str(),
                docs[0].chunk_count,
                docs[0].page_count
            ),
            ("Contrato", 1, Some(2))
        );
        assert_eq!(docs[0].status, DocumentStatus::Embedding);
        assert!(db.unfinished().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn status_changes_update_document_and_job() {
        let path = temp_db();
        let db = Database::open(&path).unwrap();
        let InsertOutcome::Inserted(id) = db.insert(new_doc('d')).await.unwrap() else {
            panic!()
        };
        db.set_status(id, DocumentStatus::Extracting, None)
            .await
            .unwrap();
        db.set_status(
            id,
            DocumentStatus::Failed,
            Some("PDF protegido por senha".into()),
        )
        .await
        .unwrap();

        let docs = db.list().await.unwrap();
        assert_eq!(docs[0].status, DocumentStatus::Failed);
        assert_eq!(docs[0].error.as_deref(), Some("PDF protegido por senha"));
        let conn = connection::open(&path).unwrap();
        let (stage, attempts, finished): (String, i64, Option<String>) = conn
            .query_row(
                "SELECT stage, attempts, finished_at FROM embedding_jobs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((stage.as_str(), attempts), ("failed", 1));
        assert!(finished.is_some());

        assert!(
            db.set_status(9999, DocumentStatus::Extracting, None)
                .await
                .is_err(),
            "unknown document"
        );
    }
}

mod vectors {
    use nlmx_application::ports::VectorStore;
    use nlmx_domain::vectors::{DeleteScope, EmbeddingSpace, VectorError, VectorFilter};

    use super::{count, temp_db};
    use crate::{Database, connection};

    fn space(model: &str, dims: u32) -> EmbeddingSpace {
        EmbeddingSpace {
            model_id: model.into(),
            revision: "r1".into(),
            dimensions: dims,
            context_length: 512,
        }
    }

    /// Two documents: chunks 1–3 belong to document 1, chunks 4–6 to document 2.
    fn seeded() -> (Database, std::path::PathBuf) {
        let path = temp_db();
        let db = Database::open(&path).unwrap();
        let conn = connection::open(&path).unwrap();
        for doc in 1..=2 {
            conn.execute(
                "INSERT INTO documents (id, sha256, original_filename, library_path, file_size) VALUES (?1, ?2, 'x.pdf', 'x', 1)",
                rusqlite::params![doc, doc.to_string().repeat(64)],
            )
            .unwrap();
        }
        for chunk in 1..=6i64 {
            let doc = if chunk <= 3 { 1 } else { 2 };
            conn.execute(
                "INSERT INTO document_chunks (id, document_id, ordinal, text, token_count, page_start, page_end, content_hash)
                 VALUES (?1, ?2, ?1, 'texto', 1, 1, 1, ?3)",
                rusqlite::params![chunk, doc, format!("hash-{chunk}")],
            )
            .unwrap();
        }
        (db, path)
    }

    fn items() -> Vec<(i64, Vec<f32>)> {
        vec![
            (1, vec![1.0, 0.0, 0.0]),
            (2, vec![0.9, 0.1, 0.0]),
            (3, vec![0.0, 1.0, 0.0]),
            (4, vec![0.0, 0.0, 1.0]),
            (5, vec![-1.0, 0.0, 0.0]),
            (6, vec![0.7, 0.7, 0.0]),
        ]
    }

    fn ids(hits: &[nlmx_domain::vectors::VectorHit]) -> Vec<i64> {
        hits.iter().map(|h| h.chunk_id).collect()
    }

    /// Deterministic pseudo-random vectors (xorshift).
    fn random_vectors(n: usize, dims: usize, mut seed: u64) -> Vec<Vec<f32>> {
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 2001) as f32 / 1000.0 - 1.0
        };
        (0..n)
            .map(|_| (0..dims).map(|_| next()).collect())
            .collect()
    }

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (norm(a) * norm(b))
    }

    #[tokio::test]
    async fn knn_recall_matches_exact_cosine_search() {
        const N: usize = 2000;
        const DIMS: usize = 64;
        const DOCS: i64 = 20;
        let path = temp_db();
        let db = Database::open(&path).unwrap();
        {
            let mut conn = connection::open(&path).unwrap();
            let tx = conn.transaction().unwrap();
            for doc in 1..=DOCS {
                tx.execute(
                    "INSERT INTO documents (id, sha256, original_filename, library_path, file_size) VALUES (?1, ?2, 'x.pdf', 'x', 1)",
                    rusqlite::params![doc, format!("{doc:064}")],
                )
                .unwrap();
            }
            for chunk in 1..=N as i64 {
                tx.execute(
                    "INSERT INTO document_chunks (id, document_id, ordinal, text, token_count, page_start, page_end, content_hash)
                     VALUES (?1, ?2, ?1, 't', 1, 1, 1, ?3)",
                    rusqlite::params![chunk, (chunk - 1) % DOCS + 1, format!("h{chunk}")],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        let vectors = random_vectors(N, DIMS, 0x9E37_79B9_7F4A_7C15);
        let index = db
            .create_index(&space("recall", DIMS as u32))
            .await
            .unwrap();
        let items: Vec<(i64, Vec<f32>)> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (i as i64 + 1, v.clone()))
            .collect();
        for batch in items.chunks(250) {
            db.insert_batch(index.id, batch).await.unwrap();
        }
        let exact = |query: &[f32], allowed: &dyn Fn(i64) -> bool| -> Vec<i64> {
            let mut scored: Vec<(f32, i64)> = items
                .iter()
                .filter(|(id, _)| allowed(*id))
                .map(|(id, v)| (cosine(query, v), *id))
                .collect();
            scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            scored.into_iter().take(10).map(|(_, id)| id).collect()
        };
        let queries = random_vectors(25, DIMS, 42);
        let (mut found, mut expected) = (0usize, 0usize);
        for q in &queries {
            let truth = exact(q, &|_| true);
            let got = ids(&db
                .search(index.id, q, 10, &VectorFilter::default())
                .await
                .unwrap());
            found += truth.iter().filter(|id| got.contains(id)).count();
            expected += truth.len();
        }
        let recall = found as f64 / expected as f64;
        assert!(recall >= 0.99, "recall@10 = {recall}");

        // Restricted to two documents, still exact within them.
        let filter = VectorFilter {
            documents: Some(vec![3, 7]),
            chunks: None,
        };
        for q in queries.iter().take(5) {
            let truth = exact(q, &|id| matches!((id - 1) % DOCS + 1, 3 | 7));
            let got = db.search(index.id, q, 10, &filter).await.unwrap();
            assert!(got.iter().all(|h| h.document_id == 3 || h.document_id == 7));
            assert_eq!(ids(&got), truth);
        }
    }

    #[tokio::test]
    async fn knn_returns_nearest_chunks_by_cosine_distance() {
        let (db, _) = seeded();
        let index = db.create_index(&space("m", 3)).await.unwrap();
        db.insert_batch(index.id, &items()).await.unwrap();

        let all = db
            .search(index.id, &[1.0, 0.0, 0.0], 10, &VectorFilter::default())
            .await
            .unwrap();
        // 1 (same), 2 (≈0.006), 6 (≈0.293), 3 and 4 tie at 1.0 (rowid order), 5 opposite (2.0).
        assert_eq!(ids(&all), [1, 2, 6, 3, 4, 5]);
        assert!(all[0].distance.abs() < 1e-6 && (all[0].similarity - 1.0).abs() < 1e-6);
        assert!(
            (all[2].similarity - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4,
            "{:?}",
            all[2]
        );
        assert!((all[5].distance - 2.0).abs() < 1e-5);
        assert_eq!(
            (all[0].document_id, all[4].document_id),
            (1, 2),
            "document → chunk relation (chunk 1 → doc 1, chunk 4 → doc 2)"
        );
        assert!(all.windows(2).all(|w| w[0].distance <= w[1].distance));

        // Magnitude does not matter for cosine.
        let scaled = db
            .search(index.id, &[5.0, 0.0, 0.0], 2, &VectorFilter::default())
            .await
            .unwrap();
        assert_eq!(ids(&scaled), [1, 2], "k respected");
    }

    #[tokio::test]
    async fn knn_can_be_restricted_to_documents() {
        let (db, _) = seeded();
        let index = db.create_index(&space("m", 3)).await.unwrap();
        db.insert_batch(index.id, &items()).await.unwrap();

        let only_doc2 = VectorFilter {
            documents: Some(vec![2]),
            ..Default::default()
        };
        let hits = db
            .search(index.id, &[1.0, 0.0, 0.0], 2, &only_doc2)
            .await
            .unwrap();
        assert_eq!(ids(&hits), [6, 4], "k applies after the filter");
        assert!(hits.iter().all(|h| h.document_id == 2));

        let both = VectorFilter {
            documents: Some(vec![1, 2]),
            ..Default::default()
        };
        assert_eq!(
            db.search(index.id, &[1.0, 0.0, 0.0], 3, &both)
                .await
                .unwrap()
                .len(),
            3
        );
        let none = VectorFilter {
            documents: Some(vec![]),
            ..Default::default()
        };
        assert!(
            db.search(index.id, &[1.0, 0.0, 0.0], 3, &none)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn the_dimension_comes_from_the_space_not_the_code() {
        let (db, _) = seeded();
        for dims in [4u32, 16] {
            let index = db
                .create_index(&space(&format!("m{dims}"), dims))
                .await
                .unwrap();
            assert_eq!(index.space.dimensions, dims);
            let mut v = vec![0.0f32; dims as usize];
            v[dims as usize - 1] = 1.0;
            db.insert_embedding(index.id, 1, &v).await.unwrap();
            let hits = db
                .search(index.id, &v, 1, &VectorFilter::default())
                .await
                .unwrap();
            assert_eq!(ids(&hits), [1]);
            let err = db
                .insert_embedding(index.id, 2, &[1.0, 0.0, 0.0])
                .await
                .unwrap_err();
            assert_eq!(
                err,
                VectorError::DimensionMismatch {
                    expected: dims,
                    actual: 3
                }
            );
            let err = db
                .search(index.id, &[1.0], 1, &VectorFilter::default())
                .await
                .unwrap_err();
            assert!(matches!(err, VectorError::DimensionMismatch { .. }));
        }
    }

    #[tokio::test]
    async fn invalid_vectors_are_rejected_and_batches_are_atomic() {
        let (db, _) = seeded();
        let index = db.create_index(&space("m", 3)).await.unwrap();
        assert!(matches!(
            db.insert_embedding(index.id, 1, &[f32::NAN, 0.0, 0.0])
                .await,
            Err(VectorError::InvalidVector(_))
        ));
        assert!(matches!(
            db.insert_embedding(index.id, 1, &[]).await,
            Err(VectorError::InvalidVector(_))
        ));
        assert_eq!(
            db.insert_embedding(index.id, 999, &[1.0, 0.0, 0.0]).await,
            Err(VectorError::UnknownChunk(999))
        );

        let mut batch = items();
        batch[4].1 = vec![1.0, 2.0]; // one bad item
        assert!(db.insert_batch(index.id, &batch).await.is_err());
        assert_eq!(db.count(index.id).await.unwrap(), 0, "nothing written");
        assert_eq!(
            db.search(index.id, &[1.0, 0.0, 0.0], 5, &VectorFilter::default())
                .await
                .unwrap(),
            vec![]
        );
        assert_eq!(db.count(12345).await, Err(VectorError::UnknownIndex(12345)));
    }

    #[tokio::test]
    async fn inserting_again_replaces_the_vector() {
        let (db, _) = seeded();
        let index = db.create_index(&space("m", 3)).await.unwrap();
        db.insert_batch(index.id, &items()).await.unwrap();
        db.insert_embedding(index.id, 5, &[1.0, 0.0, 0.0])
            .await
            .unwrap(); // was the opposite vector
        assert_eq!(db.count(index.id).await.unwrap(), 6);
        let top = db
            .search(index.id, &[1.0, 0.0, 0.0], 2, &VectorFilter::default())
            .await
            .unwrap();
        assert_eq!(ids(&top), [1, 5]);
    }

    #[tokio::test]
    async fn delete_and_count_keep_vectors_and_relations_in_sync() {
        let (db, path) = seeded();
        let index = db.create_index(&space("m", 3)).await.unwrap();
        db.insert_batch(index.id, &items()).await.unwrap();
        let both_sides = || {
            let conn = connection::open(&path).unwrap();
            (
                count(&conn, &format!("SELECT count(*) FROM {}", index.table)),
                count(&conn, "SELECT count(*) FROM chunk_embeddings"),
            )
        };
        assert_eq!(both_sides(), (6, 6));

        assert_eq!(
            db.delete(index.id, &DeleteScope::Chunks(vec![1, 2]))
                .await
                .unwrap(),
            2
        );
        assert_eq!(both_sides(), (4, 4));
        assert_eq!(
            db.delete(index.id, &DeleteScope::Document(2))
                .await
                .unwrap(),
            3
        );
        assert_eq!(both_sides(), (1, 1));
        assert_eq!(db.count(index.id).await.unwrap(), 1);

        db.insert_batch(index.id, &items()).await.unwrap();
        // Deleting a document removes its chunks, their vectors and the relation rows.
        connection::open(&path)
            .unwrap()
            .execute("DELETE FROM documents WHERE id = 1", [])
            .unwrap();
        assert_eq!(both_sides(), (3, 3));
        let hits = db
            .search(index.id, &[1.0, 0.0, 0.0], 10, &VectorFilter::default())
            .await
            .unwrap();
        assert!(hits.iter().all(|h| h.document_id == 2));

        assert_eq!(db.delete(index.id, &DeleteScope::All).await.unwrap(), 3);
        assert_eq!(both_sides(), (0, 0));
    }

    #[tokio::test]
    async fn indexes_are_per_model_and_create_index_is_idempotent() {
        let (db, _) = seeded();
        let a = db.create_index(&space("model-a", 3)).await.unwrap();
        assert_eq!(
            db.create_index(&space("model-a", 3)).await.unwrap().id,
            a.id
        );
        assert_eq!(
            db.create_index(&space("model-a", 4)).await.unwrap_err(),
            VectorError::DimensionMismatch {
                expected: 3,
                actual: 4
            }
        );
        let b = db.create_index(&space("model-b", 3)).await.unwrap();
        assert_ne!((a.id, &a.table), (b.id, &b.table));

        db.insert_embedding(a.id, 1, &[1.0, 0.0, 0.0])
            .await
            .unwrap();
        db.insert_embedding(b.id, 3, &[1.0, 0.0, 0.0])
            .await
            .unwrap();
        let q = [1.0, 0.0, 0.0];
        assert_eq!(
            ids(&db
                .search(a.id, &q, 5, &VectorFilter::default())
                .await
                .unwrap()),
            [1]
        );
        assert_eq!(
            ids(&db
                .search(b.id, &q, 5, &VectorFilter::default())
                .await
                .unwrap()),
            [3],
            "spaces never mix"
        );
        assert_eq!(
            db.create_index(&space("model-a", 3)).await.unwrap().count,
            1
        );
    }

    #[tokio::test]
    async fn rebuild_preserves_vectors_and_results() {
        let (db, _) = seeded();
        let index = db.create_index(&space("m", 3)).await.unwrap();
        db.insert_batch(index.id, &items()).await.unwrap();
        db.delete(index.id, &DeleteScope::Chunks(vec![3]))
            .await
            .unwrap();
        let before = db
            .search(index.id, &[0.6, 0.8, 0.0], 10, &VectorFilter::default())
            .await
            .unwrap();

        assert_eq!(db.rebuild(index.id).await.unwrap(), 5);
        let after = db
            .search(index.id, &[0.6, 0.8, 0.0], 10, &VectorFilter::default())
            .await
            .unwrap();
        assert_eq!(ids(&before), ids(&after));
        for (b, a) in before.iter().zip(&after) {
            assert!((b.distance - a.distance).abs() < 1e-6);
        }
    }
}

mod lexical {
    use nlmx_application::ports::{ChunkReader, LexicalIndex, VectorStore};
    use nlmx_domain::{
        retrieval::{LexicalQuery, PageRange, RetrievalFilter},
        vectors::{EmbeddingSpace, VectorFilter},
    };

    use super::temp_db;
    use crate::{Database, connection};

    /// Document 1 ("Contrato", collection 10): chunks 1–3 on pages 1, 2, 3.
    /// Document 2 ("Manual", no collection): chunks 4–5 on pages 1–2 and 4.
    fn corpus() -> Database {
        let path = temp_db();
        let db = Database::open(&path).unwrap();
        let conn = connection::open(&path).unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO documents (id, sha256, title, original_filename, library_path, file_size)
                 VALUES (1, '{a}', 'Contrato', 'c.pdf', 'c', 1), (2, '{b}', NULL, 'manual.pdf', 'm', 1);
             INSERT INTO collections (id, name) VALUES (10, 'Contratos');
             INSERT INTO collection_documents (collection_id, document_id) VALUES (10, 1);
             INSERT INTO document_chunks (id, document_id, ordinal, text, token_count, page_start, page_end, section_path, content_hash) VALUES
               (1, 1, 0, 'A carência para internação é de 180 dias.', 8, 1, 1, 'Cláusula 1', 'h1'),
               (2, 1, 1, 'A cobertura inclui consultas e exames.', 7, 2, 2, 'Cláusula 2', 'h2'),
               (3, 1, 2, 'Carência, carência e mais carência: prazos de carência.', 9, 3, 3, 'Cláusula 3', 'h3'),
               (4, 2, 0, 'Manual do usuário: como agendar exames.', 7, 1, 2, NULL, 'h4'),
               (5, 2, 1, 'Resolução Normativa 465 da ANS.', 6, 4, 4, NULL, 'h5');",
            a = "a".repeat(64),
            b = "b".repeat(64)
        ))
        .unwrap();
        db
    }

    async fn ids(db: &Database, query: &str, filter: &RetrievalFilter) -> Vec<i64> {
        let q = LexicalQuery::from_query(query);
        LexicalIndex::search(db, &q, 10, filter)
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.chunk_id)
            .collect()
    }

    #[tokio::test]
    async fn bm25_ranks_and_ignores_accents_and_case() {
        let db = corpus();
        let all = RetrievalFilter::default();
        // Chunk 3 repeats the term, so BM25 ranks it first.
        assert_eq!(ids(&db, "carência", &all).await, [3, 1]);
        assert_eq!(
            ids(&db, "CARENCIA", &all).await,
            [3, 1],
            "diacritics and case are ignored"
        );
        // OR semantics: any term matches.
        let mut exames = ids(&db, "exames internação", &all).await;
        exames.sort();
        assert_eq!(exames, [1, 2, 4]);
        assert_eq!(ids(&db, "normativa 465", &all).await, [5]);
        assert!(ids(&db, "inexistente", &all).await.is_empty());

        let hits = LexicalIndex::search(&db, &LexicalQuery::from_query("carência"), 10, &all)
            .await
            .unwrap();
        assert!(hits[0].bm25 < hits[1].bm25, "lower bm25 is better");
    }

    #[tokio::test]
    async fn fts_syntax_in_user_text_is_harmless() {
        let db = corpus();
        let all = RetrievalFilter::default();
        for query in [
            r#"carência" OR "*"#,
            "NEAR(carência exames)",
            "carência AND NOT",
            "-carência ^",
            "\"\"\"",
        ] {
            let q = LexicalQuery::from_query(query);
            assert!(
                LexicalIndex::search(&db, &q, 10, &all).await.is_ok(),
                "{query}"
            );
        }
        assert!(
            LexicalIndex::search(&db, &LexicalQuery::from_query("o que é?"), 10, &all)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn filters_by_document_collection_and_page() {
        let db = corpus();
        let docs = |d: Vec<i64>| RetrievalFilter {
            documents: Some(d),
            ..Default::default()
        };
        assert_eq!(ids(&db, "exames", &docs(vec![2])).await, [4]);
        assert!(ids(&db, "exames", &docs(vec![])).await.is_empty());

        let collection = RetrievalFilter {
            collections: Some(vec![10]),
            ..Default::default()
        };
        assert_eq!(ids(&db, "exames", &collection).await, [2]);

        let pages = |from, to| RetrievalFilter {
            pages: Some(PageRange { from, to }),
            ..Default::default()
        };
        assert_eq!(
            ids(&db, "exames", &pages(2, 2)).await.len(),
            2,
            "chunk 2 (page 2) and chunk 4 (pages 1–2) intersect"
        );
        assert_eq!(ids(&db, "carência", &pages(3, 10)).await, [3]);

        let combined = RetrievalFilter {
            documents: Some(vec![1, 2]),
            collections: Some(vec![10]),
            pages: Some(PageRange { from: 1, to: 2 }),
        };
        // Collection 10 keeps only document 1; pages 1–2 keep chunks 1 and 2.
        assert_eq!(
            ids_sorted(ids(&db, "exames carência", &combined).await),
            [1, 2]
        );
    }

    fn ids_sorted(mut v: Vec<i64>) -> Vec<i64> {
        v.sort();
        v
    }

    #[tokio::test]
    async fn resolves_filters_and_reads_chunks_in_order() {
        let db = corpus();
        let resolved = db
            .resolve(&RetrievalFilter {
                collections: Some(vec![10]),
                pages: Some(PageRange { from: 2, to: 4 }),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(resolved.documents, Some(vec![1]));
        assert_eq!(resolved.chunks, Some(vec![2, 3]));
        assert_eq!(
            db.resolve(&RetrievalFilter::default()).await.unwrap(),
            Default::default()
        );

        let chunks = db.get_many(&[5, 1, 999]).await.unwrap();
        assert_eq!(
            chunks.iter().map(|c| c.chunk_id).collect::<Vec<_>>(),
            [5, 1],
            "requested order, missing skipped"
        );
        assert_eq!(
            chunks[0].document_title, "manual.pdf",
            "falls back to the file name"
        );
        assert_eq!(
            (chunks[1].section.as_deref(), chunks[1].page_start),
            (Some("Cláusula 1"), 1)
        );
    }

    #[tokio::test]
    async fn knn_can_be_restricted_to_chunks() {
        let db = corpus();
        let index = db
            .create_index(&EmbeddingSpace {
                model_id: "m".into(),
                revision: "r".into(),
                dimensions: 2,
                context_length: 8,
            })
            .await
            .unwrap();
        let items: Vec<(i64, Vec<f32>)> =
            (1..=5).map(|c| (c, vec![1.0, c as f32 / 10.0])).collect();
        db.insert_batch(index.id, &items).await.unwrap();
        let filter = VectorFilter {
            chunks: Some(vec![2, 5]),
            ..Default::default()
        };
        let hits = VectorStore::search(&db, index.id, &[1.0, 0.0], 5, &filter)
            .await
            .unwrap();
        assert_eq!(hits.iter().map(|h| h.chunk_id).collect::<Vec<_>>(), [2, 5]);
        let both = VectorFilter {
            chunks: Some(vec![2, 5]),
            documents: Some(vec![2]),
        };
        assert_eq!(
            VectorStore::search(&db, index.id, &[1.0, 0.0], 5, &both)
                .await
                .unwrap()
                .iter()
                .map(|h| h.chunk_id)
                .collect::<Vec<_>>(),
            [5]
        );
    }
}

mod conversations {
    use nlmx_application::ports::{
        ConversationRepository, DocumentRepository, InsertOutcome, NewDocument,
    };
    use nlmx_domain::chat::{
        AnswerGrounding, ConversationScope, MessageSource, MessageStatus, Role,
    };

    use super::{count, temp_db};
    use crate::{Database, connection};

    async fn document(db: &Database, sha: char) -> i64 {
        let InsertOutcome::Inserted(id) = db
            .insert(NewDocument {
                sha256: sha.to_string().repeat(64),
                original_filename: "relatorio.pdf".into(),
                original_path: "/x/relatorio.pdf".into(),
                library_path: "/lib/x.pdf".into(),
                file_size: 10,
            })
            .await
            .unwrap()
        else {
            panic!("insert")
        };
        id
    }

    #[tokio::test]
    async fn honours_the_conversation_repository_contract() {
        let db = Database::open(temp_db()).unwrap();
        let doc = document(&db, 'c').await;
        nlmx_testing::conversation_repository_contract(&db, doc).await;
    }

    #[tokio::test]
    async fn removing_a_document_widens_the_scope_and_drops_its_sources() {
        let path = temp_db();
        let db = Database::open(&path).unwrap();
        let doc = document(&db, 'd').await;
        let conversation = db.create(ConversationScope::Document(doc)).await.unwrap();
        let answer = db
            .add_message(
                conversation.id,
                Role::Assistant,
                "",
                MessageStatus::Streaming,
                Some(AnswerGrounding::Documents),
            )
            .await
            .unwrap();
        let source = MessageSource {
            n: 1,
            cited: true,
            document_id: doc,
            chunk_id: Some(999_999), // a chunk that doesn't exist is stored as NULL
            document_title: "Relatório".into(),
            page_start: 1,
            page_end: 1,
            section: None,
            label: "Relatório, p. 1".into(),
            quote: "texto".into(),
            bboxes: vec![],
        };
        let refs = [nlmx_domain::chat::MessagePageRef {
            page: 1,
            document_id: doc,
            source: Some(1),
        }];
        db.finish_message(
            answer,
            nlmx_application::ports::FinishedAnswer {
                content: "ok [1] [página 1]",
                status: MessageStatus::Answered,
                error: None,
                sources: &[source],
                page_refs: &refs,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            db.message(answer).await.unwrap().unwrap().sources[0].chunk_id,
            None
        );

        let conn = connection::open(&path).unwrap();
        conn.execute("DELETE FROM documents WHERE id = ?1", [doc])
            .unwrap();
        let reloaded = ConversationRepository::get(&db, conversation.id)
            .await
            .unwrap();
        assert_eq!(reloaded.unwrap().scope, ConversationScope::Library);
        assert_eq!(count(&conn, "SELECT count(*) FROM citations"), 0);
        assert_eq!(count(&conn, "SELECT count(*) FROM message_page_refs"), 0);
        assert_eq!(
            count(&conn, "SELECT count(*) FROM messages"),
            1,
            "the answer stays"
        );
    }

    #[tokio::test]
    async fn honours_the_document_removal_contract() {
        let db = Database::open(temp_db()).unwrap();
        nlmx_testing::document_removal_contract(&db).await;
    }

    fn source(n: u32, document_id: i64) -> MessageSource {
        MessageSource {
            n,
            cited: false, // a source given to the model counts even if not cited
            document_id,
            chunk_id: None,
            document_title: "Doc".into(),
            page_start: 1,
            page_end: 1,
            section: None,
            label: "Doc, p. 1".into(),
            quote: "trecho".into(),
            bboxes: vec![],
        }
    }

    /// A question and its answer, which used `sources` (and, optionally, `[página N]` of `page_ref`).
    async fn turn(
        db: &Database,
        conversation: i64,
        sources: &[i64],
        page_ref: Option<i64>,
    ) -> (i64, i64) {
        let q = db
            .add_message(
                conversation,
                Role::User,
                "pergunta",
                MessageStatus::Answered,
                None,
            )
            .await
            .unwrap();
        let a = db
            .add_message(
                conversation,
                Role::Assistant,
                "",
                MessageStatus::Streaming,
                Some(AnswerGrounding::Documents),
            )
            .await
            .unwrap();
        let sources: Vec<MessageSource> = sources
            .iter()
            .enumerate()
            .map(|(i, &d)| source(i as u32 + 1, d))
            .collect();
        let refs: Vec<_> = page_ref
            .map(|document_id| nlmx_domain::chat::MessagePageRef {
                page: 1,
                document_id,
                source: None,
            })
            .into_iter()
            .collect();
        db.finish_message(
            a,
            nlmx_application::ports::FinishedAnswer {
                content: "resposta",
                status: MessageStatus::Answered,
                error: None,
                sources: &sources,
                page_refs: &refs,
            },
        )
        .await
        .unwrap();
        (q, a)
    }

    fn add_chunks(path: &std::path::Path, document: i64, texts: &[&str]) {
        let conn = connection::open(path).unwrap();
        for (i, text) in texts.iter().enumerate() {
            conn.execute(
                "INSERT INTO document_chunks (document_id, ordinal, text, token_count, page_start, page_end, content_hash)
                 VALUES (?1, ?2, ?3, 5, 1, 1, ?4)",
                rusqlite::params![document, i as i64, text, format!("h{document}-{i}")],
            )
            .unwrap();
        }
    }

    fn message_ids(path: &std::path::Path, conversation: i64) -> Vec<i64> {
        let conn = connection::open(path).unwrap();
        let mut stmt = conn
            .prepare("SELECT id FROM messages WHERE conversation_id = ?1 ORDER BY id")
            .unwrap();
        stmt.query_map([conversation], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[tokio::test]
    async fn removing_a_document_takes_the_history_that_used_it() {
        use nlmx_domain::ingestion::RemovalImpact;
        let path = temp_db();
        let db = Database::open(&path).unwrap();
        let (x, y) = (document(&db, 'x').await, document(&db, 'y').await);
        add_chunks(&path, x, &["carência de 180 dias", "cobertura hospitalar"]);
        add_chunks(&path, y, &["reajuste anual"]);

        // A: restricted to X. B: mixed. C: only X, unrestricted. D: restricted to Y.
        let a = db.create(ConversationScope::Document(x)).await.unwrap().id;
        turn(&db, a, &[x], None).await;
        let b = db.create(ConversationScope::Library).await.unwrap().id;
        turn(&db, b, &[x, y], None).await;
        let (kept_q, kept_a) = turn(&db, b, &[y], None).await;
        turn(&db, b, &[y], Some(x)).await; // only a `[página N]` of X
        let c = db.create(ConversationScope::Library).await.unwrap().id;
        turn(&db, c, &[x], None).await;
        let d = db.create(ConversationScope::Document(y)).await.unwrap().id;
        let d_turn = turn(&db, d, &[y], None).await;

        let expected = RemovalImpact {
            chunks: 2,
            conversations: 2, // A and C
            turns: 3,         // two in B, one in C
        };
        assert_eq!(db.removal_impact(x).await.unwrap(), expected);
        let removed = db.remove(x).await.unwrap().unwrap();
        assert_eq!(removed.sha256, "x".repeat(64));
        assert_eq!(removed.impact, expected);

        for gone in [a, c] {
            assert!(
                ConversationRepository::get(&db, gone)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(message_ids(&path, b), [kept_q, kept_a]);
        assert_eq!(message_ids(&path, d), [d_turn.0, d_turn.1]);
        assert_eq!(
            ConversationRepository::get(&db, d)
                .await
                .unwrap()
                .unwrap()
                .scope,
            ConversationScope::Document(y)
        );

        let conn = connection::open(&path).unwrap();
        for (sql, left) in [
            ("SELECT count(*) FROM documents", 1),
            ("SELECT count(*) FROM document_chunks", 1),
            (
                "SELECT count(*) FROM document_chunks_fts WHERE document_chunks_fts MATCH 'carencia'",
                0,
            ),
            (
                "SELECT count(*) FROM document_chunks_fts WHERE document_chunks_fts MATCH 'reajuste'",
                1,
            ),
            (
                "SELECT count(*) FROM embedding_jobs WHERE document_id = 1",
                0,
            ),
            ("SELECT count(*) FROM citations WHERE document_id = 1", 0),
        ] {
            assert_eq!(count(&conn, sql), left, "{sql}");
        }
    }

    /// Removed text must not survive in the database files (free pages, WAL, FTS5 segments).
    #[tokio::test]
    async fn removed_text_leaves_no_bytes_behind() {
        const CANARY: &str = "Zqxwvcanario";
        let path = temp_db();
        let db = Database::open(&path).unwrap();
        let doc = document(&db, 'k').await;
        let filler = "texto de enchimento ".repeat(200);
        add_chunks(&path, doc, &[&format!("{filler} {CANARY} {filler}")]);
        let conversation = db.create(ConversationScope::Library).await.unwrap().id;
        let (_, answer) = turn(&db, conversation, &[doc], None).await;
        let conn = connection::open(&path).unwrap();
        conn.execute(
            "UPDATE messages SET content = ?2 WHERE id = ?1",
            rusqlite::params![answer, format!("Segundo o documento, {CANARY}.")],
        )
        .unwrap();
        drop(conn);

        let files = || {
            ["", "-wal"]
                .iter()
                .filter_map(|suffix| std::fs::read(format!("{}{suffix}", path.display())).ok())
                .flatten()
                .collect::<Vec<u8>>()
        };
        let contains = |bytes: &[u8], needle: &str| {
            bytes
                .windows(needle.len())
                .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
        };
        assert!(contains(&files(), CANARY), "sanity: the text is on disk");

        db.remove(doc).await.unwrap().unwrap();
        let bytes = files();
        assert!(!contains(&bytes, CANARY), "removed text still on disk");
    }
}
