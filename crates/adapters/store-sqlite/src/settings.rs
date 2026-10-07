//! `app_settings` key/value store (values are JSON).

use nlmx_application::ports::{BoxFuture, SettingsRepository, StorageError};
use rusqlite::{OptionalExtension, params};

use crate::Database;

impl SettingsRepository for Database {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Option<String>, StorageError>> {
        let key = key.to_string();
        Box::pin(self.run(move |conn| {
            conn.query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                [&key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| StorageError::new(format!("Falha ao ler a configuração '{key}': {err}")))
        }))
    }

    fn set<'a>(&'a self, key: &'a str, json: &'a str) -> BoxFuture<'a, Result<(), StorageError>> {
        let (key, json) = (key.to_string(), json.to_string());
        Box::pin(self.run(move |conn| {
            conn.execute(
                "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![key, json],
            )
            .map(|_| ())
            .map_err(|err| {
                StorageError::new(format!("Falha ao salvar a configuração '{key}': {err}"))
            })
        }))
    }
}
