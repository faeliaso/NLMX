//! `VectorStore` over sqlite-vec: one `chunk_vectors_<embedding_model_id>` vec0 table per
//! embedding space (rowid = chunk id, `document_id` metadata column), plus the `chunk_embeddings`
//! relation (document → chunk → embedding model → vector).

use nlmx_application::ports::{BoxFuture, VectorStore};
use nlmx_domain::vectors::{
    ChunkId, DeleteScope, EmbeddingSpace, VectorError, VectorFilter, VectorHit, VectorIndex,
    VectorIndexId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};

use crate::{Database, vector};

/// Upper bound for `k` (sqlite-vec brute-force KNN; results beyond this are not useful for RAG).
pub const MAX_K: usize = 1000;

fn storage(err: rusqlite::Error) -> VectorError {
    VectorError::Storage(err.to_string())
}

fn to_blob(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_blob(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

fn validate(vector: &[f32], dims: u32) -> Result<(), VectorError> {
    if vector.is_empty() {
        return Err(VectorError::InvalidVector("vetor vazio".into()));
    }
    if vector.len() != dims as usize {
        return Err(VectorError::DimensionMismatch {
            expected: dims,
            actual: vector.len() as u32,
        });
    }
    if let Some(i) = vector.iter().position(|x| !x.is_finite()) {
        return Err(VectorError::InvalidVector(format!(
            "valor não finito na posição {i}"
        )));
    }
    Ok(())
}

struct IndexRow {
    space: EmbeddingSpace,
    table: String,
}

fn index_row(conn: &Connection, index: VectorIndexId) -> Result<IndexRow, VectorError> {
    conn.query_row(
        "SELECT model_key, revision, dims, max_tokens, vector_table FROM embedding_models WHERE id = ?1",
        [index],
        |r| {
            Ok((
                EmbeddingSpace { model_id: r.get(0)?, revision: r.get(1)?, dimensions: r.get(2)?, context_length: r.get(3)? },
                r.get::<_, Option<String>>(4)?,
            ))
        },
    )
    .optional()
    .map_err(storage)?
    .and_then(|(space, table)| table.map(|table| IndexRow { space, table }))
    // The table name is only ever derived from the integer id (never from user text).
    .filter(|row| row.table == vector::table_name(index))
    .ok_or(VectorError::UnknownIndex(index))
}

fn count(conn: &Connection, index: VectorIndexId) -> Result<u64, VectorError> {
    conn.query_row(
        "SELECT count(*) FROM chunk_embeddings WHERE embedding_model_id = ?1",
        [index],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as u64)
    .map_err(storage)
}

fn create_index(conn: &mut Connection, space: &EmbeddingSpace) -> Result<VectorIndex, VectorError> {
    if space.dimensions == 0 {
        return Err(VectorError::InvalidVector("dimensão zero".into()));
    }
    let existing: Option<(i64, u32)> = conn
        .query_row(
            "SELECT id, dims FROM embedding_models WHERE model_key = ?1 AND revision = ?2",
            params![space.model_id, space.revision],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(storage)?;
    let id = match existing {
        Some((_, dims)) if dims != space.dimensions => {
            return Err(VectorError::DimensionMismatch { expected: dims, actual: space.dimensions });
        }
        Some((id, _)) => id,
        None => conn
            .query_row(
                "INSERT INTO embedding_models (model_key, revision, display_name, dims, max_tokens, status, installed_at)
                 VALUES (?1, ?2, ?1, ?3, ?4, 'installed', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                 RETURNING id",
                params![space.model_id, space.revision, space.dimensions, space.context_length.max(1)],
                |r| r.get(0),
            )
            .map_err(storage)?,
    };
    let table = vector::ensure_table(conn, id).map_err(|e| VectorError::Storage(e.message))?;
    Ok(VectorIndex {
        id,
        space: space.clone(),
        table,
        count: count(conn, id)?,
    })
}

/// Inserts or replaces vectors inside an open transaction.
fn insert(
    tx: &Transaction<'_>,
    index: VectorIndexId,
    items: &[(ChunkId, &[f32])],
) -> Result<(), VectorError> {
    let row = index_row(tx, index)?;
    let mut chunk_info = tx
        .prepare("SELECT document_id, content_hash FROM document_chunks WHERE id = ?1")
        .map_err(storage)?;
    let mut delete_vec = tx
        .prepare(&format!("DELETE FROM {} WHERE rowid = ?1", row.table))
        .map_err(storage)?;
    let mut insert_vec = tx
        .prepare(&format!(
            "INSERT INTO {} (rowid, embedding, document_id) VALUES (?1, ?2, ?3)",
            row.table
        ))
        .map_err(storage)?;
    let mut relate = tx
        .prepare(
            "INSERT INTO chunk_embeddings (chunk_id, embedding_model_id, document_id, content_hash)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (chunk_id, embedding_model_id) DO UPDATE SET
                 document_id = excluded.document_id,
                 content_hash = excluded.content_hash,
                 embedded_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        )
        .map_err(storage)?;
    for (chunk, vector) in items {
        validate(vector, row.space.dimensions)?;
        let (document_id, content_hash): (i64, String) = chunk_info
            .query_row([chunk], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(storage)?
            .ok_or(VectorError::UnknownChunk(*chunk))?;
        // vec0 has no upsert: replace explicitly.
        delete_vec.execute([chunk]).map_err(storage)?;
        insert_vec
            .execute(params![chunk, to_blob(vector), document_id])
            .map_err(storage)?;
        relate
            .execute(params![chunk, index, document_id, content_hash])
            .map_err(storage)?;
    }
    Ok(())
}

fn insert_all(
    conn: &mut Connection,
    index: VectorIndexId,
    items: &[(ChunkId, &[f32])],
) -> Result<(), VectorError> {
    let tx = conn.transaction().map_err(storage)?;
    insert(&tx, index, items)?;
    tx.commit().map_err(storage)
}

fn search(
    conn: &Connection,
    index: VectorIndexId,
    query: &[f32],
    k: usize,
    filter: &VectorFilter,
) -> Result<Vec<VectorHit>, VectorError> {
    let row = index_row(conn, index)?;
    validate(query, row.space.dimensions)?;
    let k = k.clamp(1, MAX_K);
    let documents = filter.documents.as_deref();
    let chunks = filter.chunks.as_deref();
    if documents.is_some_and(<[i64]>::is_empty) || chunks.is_some_and(<[i64]>::is_empty) {
        return Ok(Vec::new());
    }
    // KNN with the filters applied inside the search (vec0 metadata column / rowid constraint).
    let placeholders = |n: usize| vec!["?"; n].join(", ");
    let mut clauses = String::new();
    if let Some(ids) = documents {
        clauses.push_str(&format!(
            " AND document_id IN ({})",
            placeholders(ids.len())
        ));
    }
    if let Some(ids) = chunks {
        clauses.push_str(&format!(" AND rowid IN ({})", placeholders(ids.len())));
    }
    let sql = format!(
        "SELECT rowid, document_id, distance FROM {} WHERE embedding MATCH ? AND k = ?{clauses} ORDER BY distance",
        row.table
    );
    let mut values: Vec<rusqlite::types::Value> = vec![to_blob(query).into(), (k as i64).into()];
    values.extend(documents.unwrap_or_default().iter().map(|&d| d.into()));
    values.extend(chunks.unwrap_or_default().iter().map(|&c| c.into()));
    let mut stmt = conn.prepare(&sql).map_err(storage)?;
    stmt.query_map(params_from_iter(values), |r| {
        let distance: f64 = r.get(2)?;
        Ok(VectorHit {
            chunk_id: r.get(0)?,
            document_id: r.get(1)?,
            distance: distance as f32,
            similarity: (1.0 - distance) as f32,
        })
    })
    .and_then(|rows| rows.collect::<rusqlite::Result<Vec<VectorHit>>>())
    .map(|mut hits| {
        // vec0 only allows `ORDER BY distance`; break ties by chunk id for stable results.
        hits.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then(a.chunk_id.cmp(&b.chunk_id))
        });
        hits
    })
    .map_err(storage)
}

fn delete(
    conn: &mut Connection,
    index: VectorIndexId,
    scope: &DeleteScope,
) -> Result<u64, VectorError> {
    let row = index_row(conn, index)?;
    let tx = conn.transaction().map_err(storage)?;
    let chunks: Vec<ChunkId> = match scope {
        DeleteScope::Chunks(ids) => ids.clone(),
        DeleteScope::Document(document) => {
            let mut stmt = tx
                .prepare("SELECT chunk_id FROM chunk_embeddings WHERE embedding_model_id = ?1 AND document_id = ?2")
                .map_err(storage)?;
            stmt.query_map(params![index, document], |r| r.get(0))
                .and_then(|r| r.collect())
                .map_err(storage)?
        }
        DeleteScope::All => {
            let mut stmt = tx
                .prepare("SELECT chunk_id FROM chunk_embeddings WHERE embedding_model_id = ?1")
                .map_err(storage)?;
            stmt.query_map([index], |r| r.get(0))
                .and_then(|r| r.collect())
                .map_err(storage)?
        }
    };
    let mut removed = 0u64;
    {
        let mut delete_vec = tx
            .prepare(&format!("DELETE FROM {} WHERE rowid = ?1", row.table))
            .map_err(storage)?;
        let mut delete_rel = tx
            .prepare("DELETE FROM chunk_embeddings WHERE chunk_id = ?1 AND embedding_model_id = ?2")
            .map_err(storage)?;
        for chunk in &chunks {
            delete_vec.execute([chunk]).map_err(storage)?;
            removed += delete_rel.execute(params![chunk, index]).map_err(storage)? as u64;
        }
    }
    tx.commit().map_err(storage)?;
    Ok(removed)
}

/// Recreates the vec0 table and re-inserts the vectors of chunks that still have a relation row
/// (dropping orphans on either side). Vectors are copied in memory, one index at a time.
fn rebuild(conn: &mut Connection, index: VectorIndexId) -> Result<u64, VectorError> {
    let row = index_row(conn, index)?;
    let vectors: Vec<(ChunkId, Vec<f32>)> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT v.rowid, v.embedding FROM {} v
                 JOIN chunk_embeddings e ON e.chunk_id = v.rowid AND e.embedding_model_id = ?1
                 ORDER BY v.rowid",
                row.table
            ))
            .map_err(storage)?;
        stmt.query_map([index], |r| {
            Ok((r.get(0)?, from_blob(&r.get::<_, Vec<u8>>(1)?)))
        })
        .and_then(|rows| rows.collect())
        .map_err(storage)?
    };
    vector::drop_table(conn, index).map_err(|e| VectorError::Storage(e.message))?;
    vector::ensure_table(conn, index).map_err(|e| VectorError::Storage(e.message))?;
    let tx = conn.transaction().map_err(storage)?;
    tx.execute(
        "DELETE FROM chunk_embeddings WHERE embedding_model_id = ?1",
        [index],
    )
    .map_err(storage)?;
    let items: Vec<(ChunkId, &[f32])> = vectors.iter().map(|(c, v)| (*c, v.as_slice())).collect();
    insert(&tx, index, &items)?;
    tx.commit().map_err(storage)?;
    count(conn, index)
}

impl Database {
    async fn vectors<T, F>(&self, f: F) -> Result<T, VectorError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, VectorError> + Send + 'static,
    {
        self.run(move |conn| Ok(f(conn)))
            .await
            .map_err(|e| VectorError::Storage(e.message))?
    }
}

impl VectorStore for Database {
    fn create_index<'a>(
        &'a self,
        space: &'a EmbeddingSpace,
    ) -> BoxFuture<'a, Result<VectorIndex, VectorError>> {
        let space = space.clone();
        Box::pin(self.vectors(move |conn| create_index(conn, &space)))
    }

    fn insert_embedding<'a>(
        &'a self,
        index: VectorIndexId,
        chunk: ChunkId,
        vector: &'a [f32],
    ) -> BoxFuture<'a, Result<(), VectorError>> {
        let vector = vector.to_vec();
        Box::pin(self.vectors(move |conn| insert_all(conn, index, &[(chunk, &vector)])))
    }

    fn insert_batch<'a>(
        &'a self,
        index: VectorIndexId,
        items: &'a [(ChunkId, Vec<f32>)],
    ) -> BoxFuture<'a, Result<(), VectorError>> {
        let items = items.to_vec();
        Box::pin(self.vectors(move |conn| {
            let refs: Vec<(ChunkId, &[f32])> =
                items.iter().map(|(c, v)| (*c, v.as_slice())).collect();
            insert_all(conn, index, &refs)
        }))
    }

    fn search<'a>(
        &'a self,
        index: VectorIndexId,
        query: &'a [f32],
        k: usize,
        filter: &'a VectorFilter,
    ) -> BoxFuture<'a, Result<Vec<VectorHit>, VectorError>> {
        let (query, filter) = (query.to_vec(), filter.clone());
        Box::pin(self.vectors(move |conn| search(conn, index, &query, k, &filter)))
    }

    fn delete<'a>(
        &'a self,
        index: VectorIndexId,
        scope: &'a DeleteScope,
    ) -> BoxFuture<'a, Result<u64, VectorError>> {
        let scope = scope.clone();
        Box::pin(self.vectors(move |conn| delete(conn, index, &scope)))
    }

    fn rebuild(&self, index: VectorIndexId) -> BoxFuture<'_, Result<u64, VectorError>> {
        Box::pin(self.vectors(move |conn| rebuild(conn, index)))
    }

    fn count(&self, index: VectorIndexId) -> BoxFuture<'_, Result<u64, VectorError>> {
        Box::pin(self.vectors(move |conn| {
            index_row(conn, index)?;
            count(conn, index)
        }))
    }
}
