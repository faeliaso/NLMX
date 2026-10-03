//! `LexicalIndex` over FTS5 (`document_chunks_fts`, BM25) and `ChunkReader`.

use nlmx_application::ports::{
    BoxFuture, Candidates, ChunkReader, ChunkView, LexicalIndex, StorageError,
};
use nlmx_domain::{
    retrieval::{LexicalCandidate, LexicalQuery, RetrievalFilter},
    vectors::ChunkId,
};
use rusqlite::{Connection, params_from_iter, types::Value};

use crate::Database;

fn err(action: &str) -> impl Fn(rusqlite::Error) -> StorageError + '_ {
    move |e| StorageError::new(format!("Falha ao {action}: {e}"))
}

/// SQL conditions (on `document_chunks c`) for a filter, with their bound values.
fn filter_sql(filter: &RetrievalFilter) -> (String, Vec<Value>) {
    let mut sql = String::new();
    let mut values: Vec<Value> = Vec::new();
    let list = |n: usize| vec!["?"; n].join(", ");
    if let Some(docs) = &filter.documents {
        sql.push_str(&format!(" AND c.document_id IN ({})", list(docs.len())));
        values.extend(docs.iter().map(|&d| Value::from(d)));
    }
    if let Some(collections) = &filter.collections {
        sql.push_str(&format!(
            " AND c.document_id IN (SELECT document_id FROM collection_documents WHERE collection_id IN ({}))",
            list(collections.len())
        ));
        values.extend(collections.iter().map(|&c| Value::from(c)));
    }
    if let Some(pages) = filter.pages {
        // Chunks whose page span intersects [from, to].
        sql.push_str(" AND c.page_start <= ? AND c.page_end >= ?");
        values.push(Value::from(pages.to));
        values.push(Value::from(pages.from));
    }
    (sql, values)
}

fn lexical_search(
    conn: &Connection,
    query: &LexicalQuery,
    k: usize,
    filter: &RetrievalFilter,
) -> Result<Vec<LexicalCandidate>, StorageError> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let (conditions, filter_values) = filter_sql(filter);
    let sql = format!(
        "SELECT c.id, c.document_id, bm25(document_chunks_fts) AS score
         FROM document_chunks_fts
         JOIN document_chunks c ON c.id = document_chunks_fts.rowid
         WHERE document_chunks_fts MATCH ?{conditions}
         ORDER BY score, c.id
         LIMIT ?"
    );
    let mut values = vec![Value::from(query.to_fts5())];
    values.extend(filter_values);
    values.push(Value::from(k as i64));
    let mut stmt = conn
        .prepare(&sql)
        .map_err(err("buscar no índice textual"))?;
    stmt.query_map(params_from_iter(values), |r| {
        Ok(LexicalCandidate {
            chunk_id: r.get(0)?,
            document_id: r.get(1)?,
            bm25: r.get(2)?,
        })
    })
    .and_then(|rows| rows.collect())
    .map_err(err("buscar no índice textual"))
}

fn resolve(conn: &Connection, filter: &RetrievalFilter) -> Result<Candidates, StorageError> {
    let documents = if filter.documents.is_some() || filter.collections.is_some() {
        let doc_filter = RetrievalFilter {
            pages: None,
            ..filter.clone()
        };
        let (conditions, values) = filter_sql(&doc_filter);
        // Reuse the chunk conditions on documents by aliasing `documents d` as `c` with document_id = id.
        let sql = format!(
            "SELECT DISTINCT c.document_id FROM (SELECT id AS document_id FROM documents) c WHERE 1{conditions}"
        );
        let mut stmt = conn.prepare(&sql).map_err(err("aplicar filtros"))?;
        let ids: Vec<i64> = stmt
            .query_map(params_from_iter(values), |r| r.get(0))
            .and_then(|r| r.collect())
            .map_err(err("aplicar filtros"))?;
        Some(ids)
    } else {
        None
    };
    let chunks = if filter.pages.is_some() {
        let (conditions, values) = filter_sql(filter);
        let sql = format!("SELECT c.id FROM document_chunks c WHERE 1{conditions} ORDER BY c.id");
        let mut stmt = conn.prepare(&sql).map_err(err("aplicar filtros"))?;
        let ids: Vec<i64> = stmt
            .query_map(params_from_iter(values), |r| r.get(0))
            .and_then(|r| r.collect())
            .map_err(err("aplicar filtros"))?;
        Some(ids)
    } else {
        None
    };
    Ok(Candidates { documents, chunks })
}

/// Parses the stored `bboxes` JSON (`[{page, left, top, right, bottom}]`); invalid entries are skipped.
pub(crate) fn parse_boxes(json: &str) -> Vec<nlmx_domain::ingestion::PageBox> {
    #[derive(serde::Deserialize)]
    struct Stored {
        page: u32,
        left: f32,
        top: f32,
        right: f32,
        bottom: f32,
    }
    serde_json::from_str::<Vec<Stored>>(json)
        .unwrap_or_default()
        .into_iter()
        .map(|b| nlmx_domain::ingestion::PageBox {
            page: b.page,
            bbox: nlmx_domain::document::BoundingBox {
                left: b.left,
                top: b.top,
                right: b.right,
                bottom: b.bottom,
            },
        })
        .collect()
}

const CHUNK_VIEW: &str =
    "SELECT c.id, c.document_id, coalesce(d.title, d.original_filename), c.page_start, c.page_end,
            c.section_path, c.text, c.bboxes, c.ordinal, c.content_hash
     FROM document_chunks c JOIN documents d ON d.id = c.document_id";

fn chunk_view(r: &rusqlite::Row<'_>) -> rusqlite::Result<ChunkView> {
    Ok(ChunkView {
        chunk_id: r.get(0)?,
        document_id: r.get(1)?,
        document_title: r.get(2)?,
        ordinal: r.get(8)?,
        content_hash: r.get(9)?,
        page_start: r.get(3)?,
        page_end: r.get(4)?,
        section: r.get(5)?,
        text: r.get(6)?,
        bboxes: parse_boxes(&r.get::<_, String>(7)?),
    })
}

fn document_chunks(conn: &Connection, document: i64) -> Result<Vec<ChunkView>, StorageError> {
    let mut stmt = conn
        .prepare(&format!(
            "{CHUNK_VIEW} WHERE c.document_id = ?1 ORDER BY c.ordinal"
        ))
        .map_err(err("ler trechos"))?;
    stmt.query_map([document], chunk_view)
        .and_then(|rows| rows.collect())
        .map_err(err("ler trechos"))
}

fn get_many(conn: &Connection, ids: &[ChunkId]) -> Result<Vec<ChunkView>, StorageError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT c.id, c.document_id, coalesce(d.title, d.original_filename), c.page_start, c.page_end,
                c.section_path, c.text, c.bboxes, c.ordinal, c.content_hash
         FROM document_chunks c JOIN documents d ON d.id = c.document_id
         WHERE c.id IN ({})",
        vec!["?"; ids.len()].join(", ")
    );
    let mut stmt = conn.prepare(&sql).map_err(err("ler trechos"))?;
    let mut found: Vec<ChunkView> = stmt
        .query_map(params_from_iter(ids.iter()), |r| {
            Ok(ChunkView {
                chunk_id: r.get(0)?,
                document_id: r.get(1)?,
                document_title: r.get(2)?,
                ordinal: r.get(8)?,
                content_hash: r.get(9)?,
                page_start: r.get(3)?,
                page_end: r.get(4)?,
                section: r.get(5)?,
                text: r.get(6)?,
                bboxes: parse_boxes(&r.get::<_, String>(7)?),
            })
        })
        .and_then(|rows| rows.collect())
        .map_err(err("ler trechos"))?;
    let position = |id: ChunkId| ids.iter().position(|&x| x == id).unwrap_or(usize::MAX);
    found.sort_by_key(|c| position(c.chunk_id));
    Ok(found)
}

impl LexicalIndex for Database {
    fn search<'a>(
        &'a self,
        query: &'a LexicalQuery,
        k: usize,
        filter: &'a RetrievalFilter,
    ) -> BoxFuture<'a, Result<Vec<LexicalCandidate>, StorageError>> {
        let (query, filter) = (query.clone(), filter.clone());
        Box::pin(self.run(move |conn| lexical_search(conn, &query, k, &filter)))
    }
}

impl ChunkReader for Database {
    fn resolve<'a>(
        &'a self,
        filter: &'a RetrievalFilter,
    ) -> BoxFuture<'a, Result<Candidates, StorageError>> {
        let filter = filter.clone();
        Box::pin(self.run(move |conn| resolve(conn, &filter)))
    }

    fn document_chunks(
        &self,
        document: i64,
    ) -> BoxFuture<'_, Result<Vec<ChunkView>, StorageError>> {
        Box::pin(self.run(move |conn| document_chunks(conn, document)))
    }

    fn get_many<'a>(
        &'a self,
        ids: &'a [ChunkId],
    ) -> BoxFuture<'a, Result<Vec<ChunkView>, StorageError>> {
        let ids = ids.to_vec();
        Box::pin(self.run(move |conn| get_many(conn, &ids)))
    }

    fn chunked_documents(&self) -> BoxFuture<'_, Result<Vec<i64>, StorageError>> {
        Box::pin(self.run(|conn| {
            let mut stmt = conn
                .prepare("SELECT DISTINCT document_id FROM document_chunks ORDER BY document_id")
                .map_err(err("listar documentos"))?;
            stmt.query_map([], |r| r.get(0))
                .and_then(|rows| rows.collect())
                .map_err(err("listar documentos"))
        }))
    }
}
