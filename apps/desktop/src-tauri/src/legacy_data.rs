//! One-time move of the data directory left by the app's previous name (PDF RAG, bundle id
//! `dev.pdfrag.desktop`) to the current one. Runs in `setup` before telemetry and the database
//! open anything, so nothing holds the old files.
//!
//! Besides the move, it renames the files that carried the old name and rewrites the absolute
//! paths stored inside the directory (`documents.library_path`, `model_path` in
//! `embedding.json`); `run/` is dropped because its pidfiles point at the old app's binaries.

use std::path::{Path, PathBuf};

use nlmx_store_sqlite::{DATABASE_FILE, Database};

/// Bundle id (= data directory name) of the app before it was renamed to NLMX.
const LEGACY_IDENTIFIER: &str = "dev.pdfrag.desktop";
const LEGACY_DATABASE_FILE: &str = "pdfrag.sqlite3";
const LEGACY_LOG_FILE: &str = "pdfrag.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    NotNeeded,
    Migrated,
    /// The directory could not be moved; the old one is left untouched.
    MoveFailed,
    /// The directory moved, but a later step failed (the app still opens).
    Incomplete,
}

impl Outcome {
    /// Logged once telemetry is up. Never includes paths.
    pub fn log(self) {
        match self {
            Outcome::NotNeeded => {}
            Outcome::Migrated => {
                tracing::info!("data directory migrated from the previous app name")
            }
            Outcome::MoveFailed => {
                tracing::error!("could not move the data directory of the previous app name")
            }
            Outcome::Incomplete => {
                tracing::warn!("data directory moved, but some stored paths were not updated")
            }
        }
    }
}

/// Moves `<parent of data_dir>/dev.pdfrag.desktop` to `data_dir` when only the old one exists.
pub fn migrate(data_dir: &Path) -> Outcome {
    let Some(parent) = data_dir.parent() else {
        return Outcome::NotNeeded;
    };
    migrate_from(&parent.join(LEGACY_IDENTIFIER), data_dir)
}

fn migrate_from(old: &Path, new: &Path) -> Outcome {
    if new.exists() || !old.is_dir() {
        return Outcome::NotNeeded;
    }
    if std::fs::rename(old, new).is_err() {
        return Outcome::MoveFailed;
    }
    let renamed = rename_legacy_files(new).is_ok();
    let _ = std::fs::remove_dir_all(new.join("run"));
    let database = rebase_database(new, old).is_ok();
    let embedding = rebase_embedding_config(&new.join("embedding.json"), old, new).is_ok();
    if renamed && database && embedding {
        Outcome::Migrated
    } else {
        Outcome::Incomplete
    }
}

fn rename_legacy_files(dir: &Path) -> std::io::Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        rename_if_present(
            &dir.join(format!("{LEGACY_DATABASE_FILE}{suffix}")),
            &dir.join(format!("{DATABASE_FILE}{suffix}")),
        )?;
    }
    let logs = dir.join("logs");
    let Ok(entries) = std::fs::read_dir(&logs) else {
        return Ok(());
    };
    let log_file = nlmx_telemetry::LOG_FILE;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // pdfrag.jsonl and its rotations pdfrag.jsonl.1 … .N
        if let Some(rest) = name.strip_prefix(LEGACY_LOG_FILE) {
            rename_if_present(&entry.path(), &logs.join(format!("{log_file}{rest}")))?;
        }
    }
    Ok(())
}

fn rename_if_present(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn rebase_database(dir: &Path, old: &Path) -> Result<(), ()> {
    let path = dir.join(DATABASE_FILE);
    if !path.exists() {
        return Ok(());
    }
    let db = Database::open(&path).map_err(|_| ())?;
    db.rebase_library_paths(old, dir)
        .map(|_| ())
        .map_err(|_| ())
}

/// Rewrites `model_path` when it points inside the old directory; other fields are kept as is.
fn rebase_embedding_config(path: &Path, old: &Path, new: &Path) -> Result<(), ()> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Ok(());
    };
    let mut config: serde_json::Value = serde_json::from_str(&raw).map_err(|_| ())?;
    let Some(model_path) = config.get("model_path").and_then(|v| v.as_str()) else {
        return Ok(());
    };
    let Ok(relative) = Path::new(model_path).strip_prefix(old) else {
        return Ok(());
    };
    let rebased: PathBuf = new.join(relative);
    config["model_path"] = serde_json::Value::String(rebased.display().to_string());
    let json = serde_json::to_string_pretty(&config).map_err(|_| ())?;
    std::fs::write(path, json).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn temp_parent() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "nlmx-legacy-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An old data directory with a database (one document in the library), logs, run files
    /// and an active model.
    fn legacy_dir(parent: &Path) -> PathBuf {
        let old = parent.join(LEGACY_IDENTIFIER);
        std::fs::create_dir_all(old.join("logs")).unwrap();
        std::fs::create_dir_all(old.join("run")).unwrap();
        std::fs::create_dir_all(old.join("library")).unwrap();
        std::fs::write(old.join("run/llama-server.pid"), "1").unwrap();
        std::fs::write(old.join("logs/pdfrag.jsonl"), "{}\n").unwrap();
        std::fs::write(old.join("logs/pdfrag.jsonl.1"), "{}\n").unwrap();
        std::fs::write(old.join("logs/llama-server.log"), "").unwrap();
        let model = old.join("models/m/1/m.gguf");
        std::fs::write(
            old.join("embedding.json"),
            serde_json::json!({ "model_id": "m", "model_path": model, "context_size": 8192 })
                .to_string(),
        )
        .unwrap();

        let db = Database::open(old.join(LEGACY_DATABASE_FILE)).unwrap();
        let library = old.join("library/a.pdf").display().to_string();
        let tokio = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        tokio.block_on(async {
            use nlmx_application::ports::{DocumentRepository, NewDocument};
            db.insert(NewDocument {
                sha256: "a".repeat(64),
                original_filename: "a.pdf".into(),
                original_path: "/elsewhere/a.pdf".into(),
                library_path: library,
                file_size: 1,
                document_type: nlmx_domain::document_type::DocumentType::Pdf,
                note_text: None,
            })
            .await
            .unwrap();
        });
        old
    }

    /// The only document's library path, read through the repository port.
    fn library_path(db_path: &Path) -> String {
        use nlmx_application::ports::DocumentRepository;
        let db = Database::open(db_path).unwrap();
        let tokio = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        tokio.block_on(async {
            let id = db.list().await.unwrap()[0].id;
            db.get(id).await.unwrap().unwrap().library_path
        })
    }

    #[test]
    fn moves_the_old_directory_and_rewrites_stored_paths() {
        let parent = temp_parent();
        let old = legacy_dir(&parent);
        let new = parent.join("dev.nlmx.desktop");

        assert_eq!(migrate(&new), Outcome::Migrated);

        assert!(!old.exists());
        assert!(new.join(DATABASE_FILE).is_file());
        assert!(!new.join(LEGACY_DATABASE_FILE).exists());
        assert!(new.join("logs/nlmx.jsonl").is_file());
        assert!(new.join("logs/nlmx.jsonl.1").is_file());
        assert!(new.join("logs/llama-server.log").is_file());
        assert!(!new.join("run").exists());
        assert_eq!(
            library_path(&new.join(DATABASE_FILE)),
            new.join("library/a.pdf").display().to_string()
        );
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(new.join("embedding.json")).unwrap())
                .unwrap();
        assert_eq!(
            config["model_path"],
            new.join("models/m/1/m.gguf").display().to_string()
        );
        assert_eq!(config["context_size"], 8192);
    }

    #[test]
    fn does_nothing_when_the_new_directory_exists() {
        let parent = temp_parent();
        let old = legacy_dir(&parent);
        let new = parent.join("dev.nlmx.desktop");
        std::fs::create_dir_all(&new).unwrap();

        assert_eq!(migrate(&new), Outcome::NotNeeded);
        assert!(old.join(LEGACY_DATABASE_FILE).is_file());
    }

    #[test]
    fn does_nothing_without_an_old_directory() {
        let parent = temp_parent();
        let new = parent.join("dev.nlmx.desktop");

        assert_eq!(migrate(&new), Outcome::NotNeeded);
        assert!(!new.exists());
    }

    #[test]
    fn keeps_a_model_path_outside_the_old_directory() {
        let parent = temp_parent();
        let config = parent.join("embedding.json");
        std::fs::write(&config, r#"{"model_path":"/models/x.gguf"}"#).unwrap();

        rebase_embedding_config(&config, &parent.join("old"), &parent.join("new")).unwrap();

        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            r#"{"model_path":"/models/x.gguf"}"#
        );
    }
}
