//! `IndexingReader`: documents with their latest ingest job, and vector counts per model.

use nlmx_application::ports::{BoxFuture, IndexingReader, StorageError};
use nlmx_domain::{
    indexing::{EmbeddedModel, IndexJob, IndexSnapshot},
    ingestion::DocumentStatus,
};
use rusqlite::{Connection, OptionalExtension};

use crate::Database;

fn err(e: rusqlite::Error) -> StorageError {
    StorageError::new(format!("Falha ao ler o estado da indexação: {e}"))
}

fn jobs(conn: &Connection) -> rusqlite::Result<Vec<IndexJob>> {
    let mut stmt = conn.prepare(
        "SELECT d.id, coalesce(d.title, d.original_filename), d.status, d.error,
                (SELECT count(*) FROM document_chunks c WHERE c.document_id = d.id),
                coalesce(j.attempts, 0), j.started_at, j.finished_at,
                CAST(round((julianday(j.finished_at) - julianday(j.started_at)) * 86400000) AS INTEGER),
                d.format
         FROM documents d
         LEFT JOIN embedding_jobs j ON j.id = (
             SELECT max(id) FROM embedding_jobs WHERE document_id = d.id AND kind = 'ingest')
         ORDER BY d.imported_at DESC, d.id DESC",
    )?;
    stmt.query_map([], |r| {
        let status: String = r.get(2)?;
        Ok(IndexJob {
            document_id: r.get(0)?,
            title: r.get(1)?,
            document_type: crate::documents::parse_type(r.get(9)?)?,
            status: DocumentStatus::parse(&status).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    format!("status desconhecido: {status}").into(),
                )
            })?,
            error: r.get(3)?,
            chunks: r.get(4)?,
            attempts: r.get(5)?,
            started_at: r.get(6)?,
            finished_at: r.get(7)?,
            // A clock change can make it negative; that is no duration.
            duration_ms: r
                .get::<_, Option<i64>>(8)?
                .and_then(|ms| u64::try_from(ms).ok()),
        })
    })?
    .collect()
}

/// The latest space of `model_id` (a model can have several revisions) and its vector count.
fn model(conn: &Connection, model_id: &str) -> rusqlite::Result<Option<EmbeddedModel>> {
    conn.query_row(
        "SELECT m.dims, (SELECT count(*) FROM chunk_embeddings e WHERE e.embedding_model_id = m.id)
         FROM embedding_models m WHERE m.model_key = ?1 ORDER BY m.id DESC LIMIT 1",
        [model_id],
        |r| {
            Ok(EmbeddedModel {
                model_id: model_id.to_string(),
                dimensions: r.get(0)?,
                chunks: r.get(1)?,
            })
        },
    )
    .optional()
}

impl IndexingReader for Database {
    fn snapshot(
        &self,
        model_id: Option<String>,
    ) -> BoxFuture<'_, Result<IndexSnapshot, StorageError>> {
        Box::pin(self.run(move |conn| {
            let jobs = jobs(conn).map_err(err)?;
            let model = match model_id {
                Some(id) => model(conn, &id).map_err(err)?,
                None => None,
            };
            Ok(IndexSnapshot {
                chunks: jobs.iter().map(|j| j.chunks).sum(),
                jobs,
                model,
            })
        }))
    }
}
