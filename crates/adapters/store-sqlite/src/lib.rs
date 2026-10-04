//! SQLite (WAL) persistence: repositories, `IngestionStore`, `LexicalIndex` (FTS5),
//! `VectorIndex` (sqlite-vec) and `JobStore`. Schema and migrations (`migrations/`) are
//! private to this crate (ADR 0004).
//!
//! Implemented so far: schema + reversible migrations, sqlite-vec registration and per-model
//! vector tables, settings, diagnostics and the document repository (ingestion).

mod connection;
mod conversations;
mod documents;
mod indexing;
mod lexical;
mod migrations;
mod settings;
mod vector;
mod vectors;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use nlmx_application::ports::{BoxFuture, StorageDiagnostics, StorageError, StorageInfo};
use rusqlite::Connection;

pub use migrations::LATEST_VERSION;
pub use vectors::MAX_K;

/// File name of the database inside the app data directory.
pub const DATABASE_FILE: &str = "nlmx.sqlite3";

/// Handle to the app database. Cheap to clone; all access goes through one writer connection
/// on a blocking thread. A read pool will be added when concurrent queries (search) exist.
#[derive(Clone)]
pub struct Database {
    path: PathBuf,
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Opens (creating if needed) the database at `path` and migrates it to the latest schema.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path = path.as_ref().to_path_buf();
        let mut conn =
            connection::open(&path).map_err(|err| storage_error("abrir o banco", err))?;
        migrations::migrate_to_latest(&mut conn)?;
        Ok(Self {
            path,
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Rewrites `documents.library_path` values under `old` to the same file under `new`, after
    /// the data directory moved. Paths elsewhere are left alone. Returns how many changed.
    /// Synchronous: called once at startup, before the async runtime serves anything.
    pub fn rebase_library_paths(&self, old: &Path, new: &Path) -> Result<usize, StorageError> {
        let prefix = format!("{}/", old.display());
        let replacement = format!("{}/", new.display());
        let conn = self
            .conn
            .lock()
            .map_err(|_| StorageError::new("conexão com o banco indisponível"))?;
        conn.execute(
            "UPDATE documents
                SET library_path = ?2 || substr(library_path, length(?1) + 1)
              WHERE substr(library_path, 1, length(?1)) = ?1",
            rusqlite::params![prefix, replacement],
        )
        .map_err(|err| storage_error("atualizar os caminhos da biblioteca", err))
    }

    /// Runs `f` with the connection on a blocking thread.
    async fn run<T, F>(&self, f: F) -> Result<T, StorageError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StorageError> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut conn = conn.lock().map_err(|_| {
                StorageError::new("conexão com o banco indisponível após uma falha anterior")
            })?;
            f(&mut conn)
        })
        .await
        .map_err(|err| StorageError::new(format!("tarefa do banco falhou: {err}")))?
    }

    /// Creates the sqlite-vec table for an embedding model (idempotent) and returns its name.
    pub async fn ensure_vector_table(
        &self,
        embedding_model_id: i64,
    ) -> Result<String, StorageError> {
        self.run(move |conn| vector::ensure_table(conn, embedding_model_id))
            .await
    }

    /// Drops an embedding model's vector table and its sync trigger.
    pub async fn drop_vector_table(&self, embedding_model_id: i64) -> Result<(), StorageError> {
        self.run(move |conn| vector::drop_table(conn, embedding_model_id))
            .await
    }
}

impl StorageDiagnostics for Database {
    fn info(&self) -> BoxFuture<'_, Result<StorageInfo, StorageError>> {
        let path = self.path.display().to_string();
        Box::pin(self.run(move |conn| {
            let query = |sql: &str| conn.query_row(sql, [], |row| row.get::<_, String>(0));
            Ok(StorageInfo {
                path,
                schema_version: migrations::current_version(conn)?,
                latest_schema_version: LATEST_VERSION,
                sqlite_version: query("SELECT sqlite_version()")
                    .map_err(|err| storage_error("ler versão", err))?,
                vector_extension_version: query("SELECT vec_version()")
                    .map_err(|err| storage_error("ler versão do sqlite-vec", err))?,
            })
        }))
    }
}

fn storage_error(action: &str, err: impl std::fmt::Display) -> StorageError {
    StorageError::new(format!("Não foi possível {action}: {err}"))
}

#[cfg(test)]
mod tests;
