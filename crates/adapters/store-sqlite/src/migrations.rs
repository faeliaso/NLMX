//! Schema migrations. Each step has an `up` and a `down` script; the applied version is kept in
//! `PRAGMA user_version`.

use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

use nlmx_application::ports::StorageError;

macro_rules! migration {
    ($dir:literal) => {
        M::up(include_str!(concat!("../migrations/", $dir, "/up.sql"))).down(include_str!(concat!(
            "../migrations/",
            $dir,
            "/down.sql"
        )))
    };
}

const STEPS: &[M<'static>] = &[
    migration!("0001_app_settings"),
    migration!("0002_documents"),
    migration!("0003_collections"),
    migration!("0004_embeddings"),
    migration!("0005_conversations"),
    migration!("0006_chunk_embeddings"),
    migration!("0007_chat"),
    migration!("0008_page_refs"),
    migration!("0009_secure_delete"),
    migration!("0010_free_chat"),
    migration!("0011_multiformat"),
    migration!("0012_citation_provenance"),
];

pub const LATEST_VERSION: u32 = STEPS.len() as u32;

pub fn migrations() -> Migrations<'static> {
    Migrations::from_slice(STEPS)
}

pub fn migrate_to_latest(conn: &mut Connection) -> Result<(), StorageError> {
    migrations()
        .to_latest(conn)
        .map_err(|err| StorageError::new(format!("Falha ao migrar o banco: {err}")))
}

/// Migrates up or down to `version` (0 = empty schema).
#[cfg(test)]
pub fn migrate_to(conn: &mut Connection, version: u32) -> Result<(), StorageError> {
    migrations()
        .to_version(conn, version as usize)
        .map_err(|err| StorageError::new(format!("Falha ao migrar para a versão {version}: {err}")))
}

pub fn current_version(conn: &Connection) -> Result<u32, StorageError> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|err| {
            StorageError::new(format!("Não foi possível ler a versão do esquema: {err}"))
        })
}
