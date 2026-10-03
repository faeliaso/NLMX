//! `DocumentRepository`: documents, pages, chunks and their ingest job.

use nlmx_application::ports::{
    BoxFuture, DocumentRecord, DocumentRepository, Extraction, InsertOutcome, NewDocument,
    PageRecord, StorageError,
};
use nlmx_domain::ingestion::{
    ChunkDraft, DocumentId, DocumentStatus, DocumentSummary, SECTION_SEPARATOR,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::Database;

fn err(action: &str) -> impl Fn(rusqlite::Error) -> StorageError + '_ {
    move |e| StorageError::new(format!("Falha ao {action}: {e}"))
}

fn parse_status(value: String) -> rusqlite::Result<DocumentStatus> {
    DocumentStatus::parse(&value).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("status desconhecido: {value}").into(),
        )
    })
}

/// The ingest job mirrors the document status (no embedding model yet ⇒ `waiting_model`).
fn job_stage(status: DocumentStatus) -> &'static str {
    match status {
        DocumentStatus::Queued => "queued",
        DocumentStatus::Extracting => "extracting",
        DocumentStatus::Structuring => "structuring",
        DocumentStatus::Chunking => "chunking",
        DocumentStatus::Embedding => "waiting_model",
        DocumentStatus::Indexed | DocumentStatus::NeedsOcr => "done",
        DocumentStatus::Failed => "failed",
    }
}

fn update_job(
    tx: &Transaction<'_>,
    id: DocumentId,
    status: DocumentStatus,
    error: Option<&str>,
) -> rusqlite::Result<()> {
    let stage = job_stage(status);
    tx.execute(
        "UPDATE embedding_jobs SET
             stage = ?2,
             error = ?3,
             attempts = attempts + (?2 = 'extracting'),
             started_at = CASE WHEN ?2 = 'extracting' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE started_at END,
             finished_at = CASE WHEN ?2 IN ('done', 'failed') THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') END,
             progress = CASE ?2 WHEN 'done' THEN 1 WHEN 'waiting_model' THEN 0.5 WHEN 'queued' THEN 0 ELSE progress END
         WHERE id = (SELECT max(id) FROM embedding_jobs WHERE document_id = ?1 AND kind = 'ingest')",
        params![id, stage, error],
    )?;
    Ok(())
}

fn set_status(
    conn: &mut Connection,
    id: DocumentId,
    status: DocumentStatus,
    error: Option<&str>,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let changed = tx.execute(
        "UPDATE documents SET status = ?2, error = ?3,
             indexed_at = CASE WHEN ?2 = 'indexed' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE indexed_at END
         WHERE id = ?1",
        params![id, status.as_str(), error],
    )?;
    if changed == 0 {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }
    update_job(&tx, id, status, error)?;
    tx.commit()
}

/// `[{"page":1,"left":72.00,"top":55.71,"right":253.03,"bottom":75.80}, …]`
fn boxes_json(chunk: &ChunkDraft) -> String {
    let items: Vec<String> = chunk
        .boxes
        .iter()
        .map(|b| {
            format!(
                r#"{{"page":{},"left":{:.2},"top":{:.2},"right":{:.2},"bottom":{:.2}}}"#,
                b.page, b.bbox.left, b.bbox.top, b.bbox.right, b.bbox.bottom
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn save_extraction(conn: &mut Connection, id: DocumentId, ex: &Extraction) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    // Re-ingestion replaces everything derived from the file (FTS and vectors follow via triggers).
    tx.execute("DELETE FROM document_chunks WHERE document_id = ?1", [id])?;
    tx.execute("DELETE FROM document_pages WHERE document_id = ?1", [id])?;
    let changed = tx.execute(
        "UPDATE documents SET title = ?2, author = ?3, pdf_created_at = ?4, page_count = ?5, has_text_layer = ?6,
             extractor_version = ?7, chunker_version = ?8, status = ?9, error = NULL
         WHERE id = ?1",
        params![
            id,
            ex.title,
            ex.author,
            ex.pdf_created_at,
            ex.page_count,
            ex.has_text_layer,
            ex.extractor_version,
            ex.chunker_version,
            ex.status.as_str()
        ],
    )?;
    if changed == 0 {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }
    {
        let mut page = tx.prepare(
            "INSERT INTO document_pages (document_id, page_number, width, height, char_count, has_text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for p in &ex.pages {
            page.execute(params![
                id,
                p.number,
                p.width,
                p.height,
                p.char_count,
                p.has_text
            ])?;
        }
        let mut chunk = tx.prepare(
            "INSERT INTO document_chunks
                 (document_id, ordinal, text, token_count, page_start, page_end, section_path, bboxes, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        for c in &ex.chunks {
            let section =
                (!c.section_path.is_empty()).then(|| c.section_path.join(SECTION_SEPARATOR));
            chunk.execute(params![
                id,
                c.index,
                c.text,
                c.token_count,
                c.page_start,
                c.page_end,
                section,
                boxes_json(c),
                c.content_hash
            ])?;
        }
    }
    update_job(&tx, id, ex.status, None)?;
    tx.commit()
}

fn insert(conn: &mut Connection, doc: &NewDocument) -> rusqlite::Result<InsertOutcome> {
    let tx = conn.transaction()?;
    let inserted: Option<DocumentId> = tx
        .query_row(
            "INSERT INTO documents (sha256, original_filename, original_path, library_path, file_size)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (sha256) DO NOTHING
             RETURNING id",
            params![doc.sha256, doc.original_filename, doc.original_path, doc.library_path, doc.file_size as i64],
            |row| row.get(0),
        )
        .optional()?;
    let outcome = match inserted {
        Some(id) => {
            tx.execute(
                "INSERT INTO embedding_jobs (document_id, kind) VALUES (?1, 'ingest')",
                [id],
            )?;
            InsertOutcome::Inserted(id)
        }
        None => InsertOutcome::AlreadyExists(tx.query_row(
            "SELECT id FROM documents WHERE sha256 = ?1",
            [&doc.sha256],
            |row| row.get(0),
        )?),
    };
    tx.commit()?;
    Ok(outcome)
}

impl DocumentRepository for Database {
    fn find_by_sha256<'a>(
        &'a self,
        sha256: &'a str,
    ) -> BoxFuture<'a, Result<Option<DocumentId>, StorageError>> {
        let sha256 = sha256.to_string();
        Box::pin(self.run(move |conn| {
            conn.query_row(
                "SELECT id FROM documents WHERE sha256 = ?1",
                [&sha256],
                |row| row.get(0),
            )
            .optional()
            .map_err(err("procurar o documento"))
        }))
    }

    fn insert(&self, document: NewDocument) -> BoxFuture<'_, Result<InsertOutcome, StorageError>> {
        Box::pin(
            self.run(move |conn| insert(conn, &document).map_err(err("registrar o documento"))),
        )
    }

    fn get(&self, id: DocumentId) -> BoxFuture<'_, Result<Option<DocumentRecord>, StorageError>> {
        Box::pin(self.run(move |conn| {
            conn.query_row(
                "SELECT id, sha256, original_filename, library_path, status, file_size FROM documents WHERE id = ?1",
                [id],
                |row| {
                    Ok(DocumentRecord {
                        id: row.get(0)?,
                        sha256: row.get(1)?,
                        original_filename: row.get(2)?,
                        library_path: row.get(3)?,
                        status: parse_status(row.get(4)?)?,
                        file_size: row.get::<_, i64>(5)?.max(0) as u64,
                    })
                },
            )
            .optional()
            .map_err(err("ler o documento"))
        }))
    }

    fn set_status(
        &self,
        id: DocumentId,
        status: DocumentStatus,
        error: Option<String>,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        Box::pin(self.run(move |conn| {
            set_status(conn, id, status, error.as_deref()).map_err(err("atualizar o status"))
        }))
    }

    fn save_extraction(
        &self,
        id: DocumentId,
        extraction: Extraction,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        Box::pin(self.run(move |conn| {
            save_extraction(conn, id, &extraction).map_err(err("salvar a extração"))
        }))
    }

    fn list(&self) -> BoxFuture<'_, Result<Vec<DocumentSummary>, StorageError>> {
        Box::pin(self.run(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT d.id, coalesce(d.title, d.original_filename), d.original_filename, d.page_count,
                            (SELECT count(*) FROM document_chunks c WHERE c.document_id = d.id),
                            d.status, d.error, d.imported_at
                     FROM documents d
                     ORDER BY d.imported_at DESC, d.id DESC",
                )
                .map_err(err("listar documentos"))?;
            stmt.query_map([], |row| {
                Ok(DocumentSummary {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    original_filename: row.get(2)?,
                    page_count: row.get(3)?,
                    chunk_count: row.get(4)?,
                    status: parse_status(row.get(5)?)?,
                    error: row.get(6)?,
                    imported_at: row.get(7)?,
                })
            })
            .and_then(|rows| rows.collect())
            .map_err(err("listar documentos"))
        }))
    }

    fn unfinished(&self) -> BoxFuture<'_, Result<Vec<DocumentId>, StorageError>> {
        Box::pin(self.run(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM documents
                     WHERE status IN ('queued', 'extracting', 'structuring', 'chunking')
                     ORDER BY id",
                )
                .map_err(err("listar documentos pendentes"))?;
            stmt.query_map([], |row| row.get(0))
                .and_then(|rows| rows.collect())
                .map_err(err("listar documentos pendentes"))
        }))
    }

    fn pages(&self, id: DocumentId) -> BoxFuture<'_, Result<Vec<PageRecord>, StorageError>> {
        Box::pin(self.run(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT page_number, coalesce(width, 612), coalesce(height, 792), char_count, has_text
                     FROM document_pages WHERE document_id = ?1 ORDER BY page_number",
                )
                .map_err(err("ler páginas"))?;
            stmt.query_map([id], |r| {
                Ok(PageRecord {
                    number: r.get(0)?,
                    width: r.get::<_, f64>(1)? as f32,
                    height: r.get::<_, f64>(2)? as f32,
                    char_count: r.get(3)?,
                    has_text: r.get::<_, i64>(4)? == 1,
                })
            })
            .and_then(|rows| rows.collect())
            .map_err(err("ler páginas"))
        }))
    }
}
