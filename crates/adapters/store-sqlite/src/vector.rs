//! sqlite-vec integration. Vector tables are created at runtime because their dimension depends on
//! the embedding model (ADR 0005); search is not implemented yet.

use std::sync::Once;

use nlmx_application::ports::StorageError;
use rusqlite::{Connection, params};

static REGISTER: Once = Once::new();

/// Registers sqlite-vec as an auto-extension for every connection opened afterwards.
#[allow(unsafe_code)]
pub fn register() {
    REGISTER.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is the extension entry point exported by the statically linked
        // sqlite-vec C library, with the signature SQLite expects for auto-extensions. Registration
        // happens once, before any connection is opened, which is what `sqlite3_auto_extension`
        // requires to be race-free.
        let rc = unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut std::os::raw::c_char,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> std::os::raw::c_int,
            >(
                sqlite_vec::sqlite3_vec_init as *const ()
            )))
        };
        assert_eq!(
            rc,
            rusqlite::ffi::SQLITE_OK,
            "failed to register sqlite-vec"
        );
    });
}

/// Table name for a model's vectors. Derived from the integer id only, so it is safe to inline in SQL.
pub fn table_name(embedding_model_id: i64) -> String {
    format!("chunk_vectors_{embedding_model_id}")
}

pub fn ensure_table(
    conn: &mut Connection,
    embedding_model_id: i64,
) -> Result<String, StorageError> {
    let err = |e: rusqlite::Error| {
        StorageError::new(format!("Falha ao preparar a tabela de vetores: {e}"))
    };
    let tx = conn.transaction().map_err(err)?;
    let dims: i64 = tx
        .query_row(
            "SELECT dims FROM embedding_models WHERE id = ?1",
            [embedding_model_id],
            |row| row.get(0),
        )
        .map_err(err)?;
    let table = table_name(embedding_model_id);
    tx.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {table} USING vec0(
             embedding float[{dims}] distance_metric=cosine,
             document_id integer
         );
         CREATE TRIGGER IF NOT EXISTS {table}_chunk_delete AFTER DELETE ON document_chunks BEGIN
             DELETE FROM {table} WHERE rowid = OLD.id;
         END;"
    ))
    .map_err(err)?;
    tx.execute(
        "UPDATE embedding_models SET vector_table = ?1 WHERE id = ?2",
        params![table, embedding_model_id],
    )
    .map_err(err)?;
    tx.commit().map_err(err)?;
    Ok(table)
}

pub fn drop_table(conn: &mut Connection, embedding_model_id: i64) -> Result<(), StorageError> {
    let err = |e: rusqlite::Error| {
        StorageError::new(format!("Falha ao remover a tabela de vetores: {e}"))
    };
    let table = table_name(embedding_model_id);
    let tx = conn.transaction().map_err(err)?;
    tx.execute_batch(&format!(
        "DROP TRIGGER IF EXISTS {table}_chunk_delete;
         DROP TABLE IF EXISTS {table};"
    ))
    .map_err(err)?;
    tx.execute(
        "UPDATE embedding_models SET vector_table = NULL WHERE id = ?1",
        [embedding_model_id],
    )
    .map_err(err)?;
    tx.commit().map_err(err)
}
